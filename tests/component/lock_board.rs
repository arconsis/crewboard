//! The board check in process, end to end against a fake GitHub: a lock whose owner agent the board
//! says is dead or has left is stale. The probe is `ops::status::agent_rows`, the same rows that
//! `crewboard status --json` prints. The scripted
//! probe cases are unit tests in `src/lock/board.rs`.
#![cfg(unix)]

use crate::support::cli::crew;
use crate::support::lock::*;
use std::path::Path;

/// `crewboard --config-dir <dir> lock take ...` against the fake board.
fn take_with_board(
    r: &Rig,
    fake: &crate::support::board::FakeBoard,
    config_dir: &Path,
    owner: &str,
    extra: &[&str],
) -> Out {
    let l = r.s("l");
    let live = r.live().to_string();
    let mut c = std::process::Command::new(BIN);
    c.arg("--config-dir")
        .arg(config_dir)
        .args(["lock", "take", &l, "--holder", owner, "--pid", &live])
        .args(extra)
        .env("HOST_LOCK_HOST", HOST)
        .env("HOST_LOCK_PS_CMD", format!("cat {}", r.s("ps.txt")))
        .env("CREWBOARD_API_BASE", fake.base())
        .env("GH_TOKEN", "ghp_faketoken")
        .env_remove("GITHUB_TOKEN")
        .env_remove("CREWBOARD_CONFIG_DIR")
        .env_remove("CREWBOARD_HOST_LOCK")
        .env_remove("CREWBOARD_HOST_LOCK_PID");
    out_of(c.output().unwrap())
}

/// Make `b` dead on the board: its heartbeat is far past 5 intervals.
fn make_dead(
    fake: &crate::support::board::FakeBoard,
    _c: &crate::support::board::Agent,
    b: &crate::support::board::Agent,
) {
    let now = crewboard::clock::now_epoch();
    fake.set_item(
        &b.item_id,
        "Heartbeat",
        &crewboard::clock::format_ts(now - 6 * 3600),
    );
}

#[test]
fn h8_e2e_a_dead_owner_agent_on_the_board_is_taken_over_an_alive_one_is_not() {
    let (fake, c, b) = crew();
    let r = Rig::new();
    // the owner agent is alive: not stale, 75
    r.plant("l", &b.name, HOST, r.live(), now());
    let o = take_with_board(&r, &fake, &c.dir, "new@x", &[]);
    assert_eq!(o.code, 75, "{}", o.all());
    assert_eq!(owner_field(&r.p("l"), "agent"), Some(b.name.clone()));
    // its heartbeat is far past the interval: dead on the board, taken over
    make_dead(&fake, &c, &b);
    let o = take_with_board(&r, &fake, &c.dir, "new@x", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stderr.contains("reason=board-dead"), "{}", o.stderr);
    assert!(
        o.stderr
            .contains(&format!("the board says agent {} is dead", b.name)),
        "{}",
        o.stderr
    );
    assert_eq!(owner_field(&r.p("l"), "agent").as_deref(), Some("new@x"));
}

#[test]
fn h8_e2e_the_same_dead_owner_is_not_judged_with_no_board() {
    let (fake, c, b) = crew();
    let r = Rig::new();
    make_dead(&fake, &c, &b);
    r.plant("l", &b.name, HOST, r.live(), now());
    let o = take_with_board(&r, &fake, &c.dir, "new@x", &["--no-board"]);
    assert_eq!(o.code, 75, "{}", o.all());
}

#[test]
fn h8b_e2e_an_owner_agent_that_has_left_the_board_is_taken_over() {
    let (fake, c, b) = crew();
    let r = Rig::new();
    fake.set_item(&b.item_id, "State", "left");
    r.plant("l", &b.name, HOST, r.live(), now());
    let o = take_with_board(&r, &fake, &c.dir, "new@x", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stderr.contains("reason=board-left"), "{}", o.stderr);
    assert!(
        o.stderr
            .contains(&format!("agent {} has left the board", b.name))
    );
}

#[test]
fn h8d_e2e_an_owner_agent_that_the_board_does_not_list_is_not_judged() {
    let (fake, c, _b) = crew();
    let r = Rig::new();
    r.plant("l", "unknown@fake-z", HOST, r.live(), now());
    let o = take_with_board(&r, &fake, &c.dir, "new@x", &[]);
    assert_eq!(o.code, 75, "{}", o.all());
}

#[test]
fn e6f_e2e_with_a_config_dir_the_check_works_and_without_one_status_says_unavailable_once() {
    let (fake, c, b) = crew();
    let r = Rig::new();
    make_dead(&fake, &c, &b);
    // --config-dir given (the global flag): board-dead takeover
    r.plant("l", &b.name, HOST, r.live(), now());
    let o = take_with_board(&r, &fake, &c.dir, "new@x", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stderr.contains("reason=board-dead"), "{}", o.stderr);
    // E6d/E6e: no config dir at all (an empty HOME, an empty cwd, no variable): `status` says it
    // once, take and wait stay silent about it
    let empty = tempfile::tempdir().unwrap();
    r.plant("m", &b.name, HOST, r.live(), now());
    let run = |sub: &str, extra: &[&str]| {
        let l = r.s("m");
        let mut cmd = std::process::Command::new(BIN);
        cmd.args(["lock", sub, &l])
            .args(extra)
            .current_dir(empty.path())
            .env("HOME", empty.path())
            .env("HOST_LOCK_HOST", HOST)
            .env("HOST_LOCK_PS_CMD", format!("cat {}", r.s("ps.txt")))
            .env("CREWBOARD_API_BASE", fake.base())
            .env("GH_TOKEN", "ghp_faketoken")
            .env_remove("GITHUB_TOKEN")
            .env_remove("CREWBOARD_CONFIG_DIR")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("CREWBOARD_HOST_LOCK");
        out_of(cmd.output().unwrap())
    };
    let s = run("status", &[]);
    assert_eq!(s.code, 0, "{}", s.all());
    assert_eq!(
        s.stderr
            .matches("board check unavailable: no agent config dir")
            .count(),
        1,
        "{}",
        s.stderr
    );
    assert!(!s.stderr.contains("board check skipped"), "{}", s.stderr);
    let t = run(
        "take",
        &[
            "--holder",
            "b@x",
            "--wait",
            "1",
            "--poll",
            "0.3",
            "--pid",
            &r.live().to_string(),
        ],
    );
    assert_eq!(t.code, 75, "{}", t.all());
    assert!(!t.stderr.contains("board check"), "{}", t.stderr);
    // and with --no-board nothing is said
    let q = run("status", &["--no-board"]);
    assert!(!q.stderr.contains("board check"), "{}", q.stderr);
}
