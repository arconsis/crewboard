//! How `crewboard lock` judges a held lock, through the real binary on real files and mtimes: the
//! age limits, empty dirs, foreign holders (a dir without a valid owner file), invalid owner
//! files, `--expect` and overdue, and the waiting list of CI. The rules are proved on in-memory
//! snapshots in `src/lock/judge.rs`; these tests prove the exit codes, the status lines, the
//! takeover lines and the files that only the binary shows. Ids are those of
//! `docs/design/testing-shell-suite-port.md`.
#![cfg(unix)]

use crate::support::lock::*;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// About ten years: older than every limit.
const LONG_AGO: u64 = 315_360_000;

/// Set the mtime of a file or a dir to `secs` ago.
fn set_age(path: &Path, secs: u64) {
    std::fs::File::open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(secs))
        .unwrap();
}

/// Rewrite `started_epoch=` of the owner file of `dir` to `secs` ago.
fn backdate(dir: &Path, secs: i64) {
    let text = owner_text(dir);
    let new: String = text
        .lines()
        .map(|l| {
            if l.starts_with("started_epoch=") {
                format!("started_epoch={}\n", now() - secs)
            } else {
                format!("{l}\n")
            }
        })
        .collect();
    std::fs::write(dir.join("owner"), new).unwrap();
}

/// Append a line to the owner file of `dir`.
fn append(dir: &Path, line: &str) {
    let mut text = owner_text(dir);
    text.push_str(line);
    text.push('\n');
    std::fs::write(dir.join("owner"), text).unwrap();
}

/// `lock take <name> --holder <holder> --no-board --pid <live>` plus `extra`.
fn take(r: &Rig, name: &str, holder: &str, extra: &[&str]) -> Out {
    let live = r.live().to_string();
    let mut a = vec![
        "take",
        name,
        "--holder",
        holder,
        "--no-board",
        "--pid",
        &live,
    ];
    let p = r.s(name);
    a[1] = &p;
    a.extend_from_slice(extra);
    r.run(&a)
}

/// `lock status <name> --no-board` plus `extra`.
fn status(r: &Rig, name: &str, extra: &[&str]) -> Out {
    let p = r.s(name);
    let mut a = vec!["status", p.as_str(), "--no-board"];
    a.extend_from_slice(extra);
    r.run(&a)
}

/// Exactly one takeover line, and it names `reason` from the list of reasons (H12).
fn one_takeover(o: &Out, reason: &str) {
    let lines: Vec<&str> = o
        .stderr
        .lines()
        .filter(|l| l.contains("took over stale lock"))
        .collect();
    assert_eq!(lines.len(), 1, "{}", o.all());
    assert!(
        [
            "pid-dead",
            "too-old",
            "board-dead",
            "board-left",
            "empty-after-grace",
            "overdue"
        ]
        .contains(&reason)
    );
    assert!(
        lines[0].contains(&format!("reason={reason} (")),
        "{}",
        lines[0]
    );
}

#[test]
fn h4_a_live_pid_older_than_max_age_is_taken_over() {
    let r = Rig::new();
    r.plant("l", "old@x", HOST, r.live(), now() - 100_000);
    let o = take(&r, "l", "new@x", &["--max-age", "3600"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stderr.contains("old, over the limit of 3600"),
        "{}",
        o.all()
    );
    one_takeover(&o, "too-old");
    assert_eq!(owner_field(&r.p("l"), "agent").as_deref(), Some("new@x"));
}

#[test]
fn h5b_another_host_that_is_too_old_is_taken_over_by_the_age_limit() {
    let r = Rig::new();
    r.plant("l", "far@y", "hostB", dead_pid(), now() - 100_000);
    let o = take(&r, "l", "new@x", &["--max-age", "3600"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stderr.contains("old owner far@y@hostB"), "{}", o.all());
    assert!(o.stderr.contains("old, over the limit"), "{}", o.all());
    one_takeover(&o, "too-old");
}

#[test]
fn h11_a_fresh_empty_dir_is_not_taken_h11b_one_older_than_the_grace_is() {
    let r = Rig::new();
    let d = r.p("l");
    std::fs::create_dir_all(&d).unwrap();
    let o = take(&r, "l", "new@x", &[]);
    assert_eq!(o.code, 75, "{}", o.all());
    assert!(d.is_dir() && !d.join("owner").exists());
    assert!(!o.stderr.contains("took over"), "{}", o.all());
    set_age(&d, LONG_AGO);
    let o = take(&r, "l", "new@x", &["--empty-grace", "30"]);
    assert_eq!(o.code, 0, "{}", o.all());
    one_takeover(&o, "empty-after-grace");
    assert_eq!(owner_field(&d, "agent").as_deref(), Some("new@x"));
}

#[test]
fn h14c_an_entry_older_than_ci_max_age_is_ignored() {
    let r = Rig::new();
    let l = r.s("l");
    assert_eq!(r.run(&["ci-wait", &l, "--holder", "ci-old"]).code, 0);
    set_age(&r.p("l.waiting/ci-old"), 7200);
    // two hours old: inside the default of 6 h it still blocks
    let o = take(&r, "l", "a@x", &[]);
    assert_eq!(o.code, 76, "{}", o.all());
    // over --ci-max-age 3600 it is ignored
    let o = take(&r, "l", "a@x", &["--ci-max-age", "3600"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(owner_field(&r.p("l"), "agent").as_deref(), Some("a@x"));
}

#[test]
fn h15_a_ci_take_passes_a_waiting_entry_and_a_running_worker() {
    let r = Rig::new();
    let l = r.s("l");
    r.set_runner(true);
    assert_eq!(r.run(&["ci-wait", &l, "--holder", "other-ci"]).code, 0);
    // the other direction: an agent is stopped by both
    assert_eq!(take(&r, "l", "a@x", &[]).code, 76);
    let o = take(&r, "l", "ci-1", &["--ci"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(owner_field(&r.p("l"), "agent").as_deref(), Some("ci-1"));
    assert!(!r.p("l.waiting/ci-1").exists());
}

#[test]
fn h19_an_entry_records_its_pid_a_dead_one_is_removed_a_live_one_blocks() {
    let r = Rig::new();
    let l = r.s("l");
    let dp = dead_pid();
    let o = r.run(&[
        "ci-wait",
        &l,
        "--holder",
        "ci-dead",
        "--pid",
        &dp.to_string(),
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    let entry = std::fs::read_to_string(r.p("l.waiting/ci-dead")).unwrap();
    assert!(entry.lines().any(|x| x == format!("pid={dp}")), "{entry}");
    // H19b: a dead pid does not block and the entry goes
    let o = take(&r, "l", "a@x", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(owner_field(&r.p("l"), "agent").as_deref(), Some("a@x"));
    assert!(!r.p("l.waiting/ci-dead").exists());
    assert_eq!(r.run(&["release", &l, "--holder", "a@x"]).code, 0);
    // H19c: a live pid blocks, and the entry stays
    let live = r.live().to_string();
    assert_eq!(
        r.run(&["ci-wait", &l, "--holder", "ci-live", "--pid", &live])
            .code,
        0
    );
    let o = take(&r, "l", "a@x", &[]);
    assert_eq!(o.code, 76, "{}", o.all());
    assert!(!r.p("l").exists());
    assert!(r.p("l.waiting/ci-live").is_file());
}

#[test]
fn h19d_an_entry_of_another_host_and_an_entry_without_a_pid_still_block() {
    let r = Rig::new();
    let w = r.p("l.waiting");
    std::fs::create_dir_all(&w).unwrap();
    std::fs::write(
        w.join("ci-far"),
        format!("name=ci-far\nhost=hostB\nstarted=x\npid={}\n", dead_pid()),
    )
    .unwrap();
    assert_eq!(take(&r, "l", "a@x", &[]).code, 76);
    std::fs::remove_file(w.join("ci-far")).unwrap();
    std::fs::write(w.join("ci-old"), "name=ci-old\nhost=hostA\nstarted=x\n").unwrap();
    assert_eq!(take(&r, "l", "a@x", &[]).code, 76);
}

#[test]
fn f1_an_old_empty_dir_is_stale_f1b_an_old_dir_with_a_file_is_held() {
    let r = Rig::new();
    let d = r.p("f1");
    std::fs::create_dir_all(&d).unwrap();
    set_age(&d, LONG_AGO);
    let o = status(&r, "f1", &[]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stdout.starts_with("stale: empty-after-grace"),
        "{}",
        o.all()
    );
    let d = r.p("f1b");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("info"), "github-actions e2e-arm64 run 4711\n").unwrap();
    set_age(&d, LONG_AGO);
    let o = status(&r, "f1b", &["--max-age", "0"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        o.stdout.trim_end(),
        "held: foreign holder: info: github-actions e2e-arm64 run 4711"
    );
}

#[test]
fn f2_a_foreign_file_older_than_the_grace_blocks_f3_older_than_max_age_it_is_taken() {
    let r = Rig::new();
    let d = r.p("f2");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("info"), "github-actions e2e-arm64 run 4711\n").unwrap();
    set_age(&d, 10);
    let o = take(&r, "f2", "a@x", &["--empty-grace", "1"]);
    assert_eq!(o.code, 75, "{}", o.all());
    assert!(!o.stderr.contains("took over"), "{}", o.all());
    let s = status(&r, "f2", &["--empty-grace", "1"]);
    assert_eq!(s.code, 0, "{}", s.all());
    assert!(d.join("info").is_file() && !d.join("owner").exists());
    // F3: --max-age 0 never makes it too old; over --max-age it is stale and taken
    let o = take(&r, "f2", "a@x", &["--empty-grace", "1", "--max-age", "0"]);
    assert_eq!(o.code, 75, "{}", o.all());
    let s = status(&r, "f2", &["--empty-grace", "1", "--max-age", "1"]);
    assert_eq!(s.code, 2, "{}", s.all());
    let o = take(&r, "f2", "a@x", &["--empty-grace", "1", "--max-age", "1"]);
    assert_eq!(o.code, 0, "{}", o.all());
    one_takeover(&o, "too-old");
    assert!(
        o.stderr
            .contains("old owner foreign holder: info: github-actions e2e-arm64 run 4711"),
        "{}",
        o.all()
    );
    assert_eq!(owner_field(&d, "agent").as_deref(), Some("a@x"));
    assert!(!d.join("info").exists());
}

#[test]
fn f4_status_and_the_wait_message_name_the_file_and_its_first_line_capped_and_clean() {
    let r = Rig::new();
    let d = r.p("f4");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("zz-info"),
        format!("job \x1b[31mred\ttab{}\nsecond line\n", "x".repeat(300)),
    )
    .unwrap();
    std::fs::write(d.join("zz-later"), "aaa\n").unwrap();
    let s = status(&r, "f4", &[]);
    let out = s.stdout.trim_end_matches('\n');
    let first = out
        .strip_prefix("held: foreign holder: zz-info: ")
        .unwrap_or_else(|| panic!("{}", s.all()));
    assert!(first.starts_with("job "), "{out}");
    assert!((100..=120).contains(&first.chars().count()), "{out}");
    assert!(
        !out.bytes().any(|b| (1..=31).contains(&b) || b == 127),
        "{out:?}"
    );
    assert!(!out.contains("second line"), "{out}");
    let o = take(&r, "f4", "a@x", &[]);
    assert!(
        o.stderr.contains("held by foreign holder: zz-info: job"),
        "{}",
        o.all()
    );
}

#[test]
fn f5_a_recorded_pid_dead_is_stale_live_blocks_another_host_is_not_judged() {
    let r = Rig::new();
    let d = r.p("f5");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("lockinfo"), format!("tool x\npid={}\n", dead_pid())).unwrap();
    let s = status(&r, "f5", &[]);
    assert_eq!(s.code, 2, "{}", s.all());
    let o = take(&r, "f5", "a@x", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    one_takeover(&o, "pid-dead");
    assert!(
        o.stderr.contains("foreign holder: lockinfo: tool x"),
        "{}",
        o.all()
    );
    // F5b: a live pid blocks
    let d = r.p("f5b");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("lockinfo"), format!("tool x\npid={}\n", r.live())).unwrap();
    let o = take(&r, "f5b", "a@x", &[]);
    assert_eq!(o.code, 75, "{}", o.all());
    assert!(d.join("lockinfo").is_file() && !d.join("owner").exists());
    // F5c: the dead pid of another host is not judged; of this host it is
    let d = r.p("f5c");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("lockinfo"),
        format!("tool x\nhost=hostB\npid={}\n", dead_pid()),
    )
    .unwrap();
    assert_eq!(take(&r, "f5c", "a@x", &[]).code, 75);
    std::fs::write(
        d.join("lockinfo"),
        format!("tool x\nhost=hostA\npid={}\n", dead_pid()),
    )
    .unwrap();
    let o = take(&r, "f5c", "a@x", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
}

#[test]
fn f6_a_dir_with_only_the_temp_file_of_a_taker_is_kept_fresh_and_empty_when_old() {
    let r = Rig::new();
    let d = r.p("f6");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("owner.tmp.123"), "agent=x\n").unwrap();
    let o = take(&r, "f6", "a@x", &[]);
    assert_eq!(o.code, 75, "{}", o.all());
    set_age(&d, LONG_AGO);
    let o = take(&r, "f6", "a@x", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    one_takeover(&o, "empty-after-grace");
}

#[test]
fn o1_an_old_one_line_owner_file_is_too_old_by_its_own_mtime_o1b_not_with_max_age_0() {
    let r = Rig::new();
    let d = r.p("o1");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("owner"), "claude@host-a\n").unwrap();
    set_age(&d.join("owner"), LONG_AGO);
    let s = status(&r, "o1", &["--max-age", "3600"]);
    assert_eq!(s.code, 2, "{}", s.all());
    let out = s.stdout.trim_end();
    assert!(
        out.starts_with("stale: too-old: the invalid owner file is ")
            && out.ends_with(
                " s old, over the limit of 3600 s; owner foreign holder: owner (no key=value fields): claude@host-a"
            ),
        "{out}"
    );
    let o = take(&r, "o1", "new@x", &["--max-age", "3600"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stderr.contains(
            "old owner foreign holder: owner (no key=value fields): claude@host-a; reason=too-old (the invalid owner file is"
        ),
        "{}",
        o.all()
    );
    one_takeover(&o, "too-old");
    assert_eq!(owner_field(&d, "agent").as_deref(), Some("new@x"));
    // O1b: --max-age 0 never makes it too old
    let d = r.p("o1b");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("owner"), "claude@host-a\n").unwrap();
    set_age(&d.join("owner"), LONG_AGO);
    let o = take(&r, "o1b", "new@x", &["--max-age", "0"]);
    assert_eq!(o.code, 75, "{}", o.all());
    assert_eq!(owner_text(&d), "claude@host-a\n");
}

#[test]
fn o2_a_fresh_one_line_owner_file_in_an_old_dir_is_held_and_named() {
    let r = Rig::new();
    let d = r.p("o2");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("owner"), "claude@host-a\n").unwrap();
    set_age(&d, LONG_AGO);
    let s = status(&r, "o2", &[]);
    assert_eq!(s.code, 0, "{}", s.all());
    assert_eq!(
        s.stdout.trim_end(),
        "held: foreign holder: owner (no key=value fields): claude@host-a"
    );
    // --wait 2: a first try that crosses a second boundary with --wait 1 ends the wait at once,
    // before the wait message
    let o = take(&r, "o2", "new@x", &["--wait", "2", "--poll", "0.3"]);
    assert_eq!(o.code, 75, "{}", o.all());
    assert!(
        o.stderr.lines().any(|l| l.contains("waiting for lock ")
            && l.contains("held by foreign holder: owner (no key=value fields): claude@host-a")),
        "{}",
        o.all()
    );
    assert!(!o.stderr.contains("took over"), "{}", o.all());
    assert_eq!(owner_text(&d), "claude@host-a\n");
}

#[test]
fn o3_an_owner_file_without_started_epoch_and_a_dead_pid_is_pid_dead() {
    let r = Rig::new();
    let d = r.p("o3");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("owner"),
        format!("agent=a@x\nhost=hostA\npid={}\n", dead_pid()),
    )
    .unwrap();
    let s = status(&r, "o3", &[]);
    assert_eq!(s.code, 2, "{}", s.all());
    let out = s.stdout.trim_end();
    assert!(
        out.starts_with("stale: pid-dead: pid ")
            && out.ends_with(
                " recorded in owner is gone; owner foreign holder: owner (no numeric started_epoch=): agent=a@x"
            ),
        "{out}"
    );
    let o = take(&r, "o3", "new@x", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    one_takeover(&o, "pid-dead");
    assert_eq!(owner_field(&d, "agent").as_deref(), Some("new@x"));
}

#[test]
fn o3b_an_invalid_owner_file_with_a_live_or_foreign_pid_is_held_and_its_problem_named() {
    let r = Rig::new();
    let d = r.p("o3b");
    std::fs::create_dir_all(&d).unwrap();
    let owner = d.join("owner");
    std::fs::write(&owner, format!("agent=a@x\nhost=hostA\npid={}\n", r.live())).unwrap();
    let s = status(&r, "o3b", &[]);
    assert_eq!(s.code, 0, "{}", s.all());
    assert_eq!(
        s.stdout.trim_end(),
        "held: foreign holder: owner (no numeric started_epoch=): agent=a@x"
    );
    assert_eq!(take(&r, "o3b", "new@x", &[]).code, 75);
    std::fs::write(
        &owner,
        format!("agent=a@x\nhost=hostB\npid={}\n", dead_pid()),
    )
    .unwrap();
    assert_eq!(take(&r, "o3b", "new@x", &[]).code, 75);
    std::fs::write(&owner, format!("agent=\nstarted_epoch={}\n", now())).unwrap();
    assert_eq!(
        status(&r, "o3b", &[]).stdout.trim_end(),
        "held: foreign holder: owner (no agent= value): agent="
    );
    std::fs::write(&owner, "started_epoch=yesterday\n").unwrap();
    assert_eq!(
        status(&r, "o3b", &[]).stdout.trim_end(),
        "held: foreign holder: owner (no agent= value, no numeric started_epoch=): started_epoch=yesterday"
    );
}

#[test]
fn o5_a_valid_owner_file_with_an_old_mtime_is_judged_by_started_epoch() {
    let r = Rig::new();
    let d = r.p("o5");
    r.plant("o5", "a@x", HOST, r.live(), now());
    set_age(&d.join("owner"), LONG_AGO);
    set_age(&d, LONG_AGO);
    let s = status(&r, "o5", &["--max-age", "3600"]);
    assert_eq!(s.code, 0, "{}", s.all());
    assert!(
        s.stdout.starts_with(&format!(
            "held: a@x@hostA pid={} since planted (planted) alive=yes age=",
            r.live()
        )),
        "{}",
        s.all()
    );
    assert_eq!(take(&r, "o5", "new@x", &["--max-age", "3600"]).code, 75);
}

#[test]
fn o6_an_owner_that_is_a_directory_is_held_fresh_and_too_old_by_its_mtime() {
    let r = Rig::new();
    let d = r.p("o6");
    std::fs::create_dir_all(d.join("owner")).unwrap();
    let s = status(&r, "o6", &[]);
    assert_eq!(s.code, 0, "{}", s.all());
    assert_eq!(
        s.stdout.trim_end(),
        "held: foreign holder: owner (not a regular file)"
    );
    set_age(&d.join("owner"), LONG_AGO);
    let o = take(&r, "o6", "new@x", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stderr
            .contains("old owner foreign holder: owner (not a regular file); reason=too-old"),
        "{}",
        o.all()
    );
    assert!(d.join("owner").is_file());
    assert_eq!(owner_field(&d, "agent").as_deref(), Some("new@x"));
}

#[test]
fn e2_expect_1_is_held_for_290_s_and_overdue_after_310_s() {
    let r = Rig::new();
    let d = r.p("e2");
    let o = r.run(&[
        "take",
        &r.s("e2"),
        "--holder",
        "a@x",
        "--no-board",
        "--pid",
        "0",
        "--expect",
        "1",
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    // the deadline is expect x 2 + 5 min = 302 s
    backdate(&d, 290);
    assert_eq!(status(&r, "e2", &[]).code, 0);
    let o = r.run(&["take", &r.s("e2"), "--holder", "b@x", "--no-board"]);
    assert_eq!(o.code, 75, "{}", o.all());
    assert_eq!(owner_field(&d, "agent").as_deref(), Some("a@x"));
    // E2b
    backdate(&d, 310);
    let s = status(&r, "e2", &[]);
    assert_eq!(s.code, 2, "{}", s.all());
    assert!(s.stdout.starts_with("stale: overdue"), "{}", s.all());
    let o = r.run(&["take", &r.s("e2"), "--holder", "b@x", "--no-board"]);
    assert_eq!(o.code, 0, "{}", o.all());
    one_takeover(&o, "overdue");
    assert_eq!(owner_field(&d, "agent").as_deref(), Some("b@x"));
}

#[test]
fn e3_pid_0_without_expect_keeps_the_age_rule_and_a_judged_pid_is_never_overdue() {
    let r = Rig::new();
    let take_b = |name: &str| r.run(&["take", &r.s(name), "--holder", "b@x", "--no-board"]);
    plant(&r.p("e3"), "a@x", HOST, "0", now() - 2000);
    assert_eq!(take_b("e3").code, 75);
    // E3b: older than 4 h
    plant(&r.p("e3b"), "a@x", HOST, "0", now() - 20_000);
    let o = take_b("e3b");
    assert_eq!(o.code, 0, "{}", o.all());
    one_takeover(&o, "too-old");
    // E3c: a live pid with expect= is judged by the pid
    r.plant("e3c", "a@x", HOST, r.live(), now() - 5000);
    append(&r.p("e3c"), "expect=1");
    assert_eq!(take_b("e3c").code, 75);
    // E3d: the pid of another host is not judged, so expect= counts
    r.plant("e3d", "a@x", "hostB", r.live(), now() - 5000);
    append(&r.p("e3d"), "expect=1");
    let o = take_b("e3d");
    assert_eq!(o.code, 0, "{}", o.all());
    one_takeover(&o, "overdue");
}

#[test]
fn e4_status_names_pid_0_and_how_late_the_job_is() {
    let r = Rig::new();
    plant(&r.p("e4"), "a@x", HOST, "0", now() - 600);
    append(&r.p("e4"), "expect=300");
    // 260 s past expect=300: 5 min rounded up, even if the run is 40 s slow
    backdate(&r.p("e4"), 560);
    let s = status(&r, "e4", &[]);
    assert_eq!(s.code, 0, "{}", s.all());
    let all = s.all();
    assert!(
        all.contains("pid=0: a dead holder cannot be detected"),
        "{all}"
    );
    assert!(all.contains("overdue by 5 min"), "{all}");
    // E4c: before the deadline only the pid=0 note
    plant(&r.p("e4c"), "a@x", HOST, "0", now() - 10);
    append(&r.p("e4c"), "expect=3600");
    let all = status(&r, "e4c", &[]).all();
    assert!(
        all.contains("pid=0: a dead holder cannot be detected"),
        "{all}"
    );
    assert!(!all.contains("overdue"), "{all}");
    // E4d: a real pid has no pid=0 note
    r.plant("e4d", "a@x", HOST, r.live(), now() - 10);
    let all = status(&r, "e4d", &[]).all();
    assert!(!all.contains("cannot be detected"), "{all}");
}
