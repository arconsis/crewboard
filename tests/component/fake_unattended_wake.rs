//! `wake --tick --runtime`: the tick, then the model wake of the unattended coordinator
//! (spec/agents#unattended-coordinator), against the fake GitHub.
//!
//! The runtime is a shell script. It does what a model would do with the judgment file: it runs
//! `crewboard verify` for the ticket on the list. The command runs in the headless config dir,
//! so the scope and the budget decide, and the verdict gets the stamp. No real runtime starts.

#![cfg(unix)]

use crate::fake_unattended::{VERIFY, hub, key, settle_day};
use crate::support::board::Out;
use crate::support::headless::{Hub, TICKET};
use crewboard::protocol::acts::unattended_line;
use std::path::{Path, PathBuf};

/// A runtime that notes its start, copies the judgment file and runs one `verify` of the
/// ticket, with the config dir that the wake gave it.
fn runtime(dir: &Path, state: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let rt = dir.join("runtime");
    let bin = env!("CARGO_BIN_EXE_crewboard");
    std::fs::write(
        &rt,
        format!(
            "#!/bin/sh\necho \"$CREWBOARD_WAKE_REASON\" >> '{d}/starts'\ncp '{s}/judgment.txt' '{d}/judgment-seen'\n'{bin}' verify {TICKET} --body 'I read the diff: the demo runs' > '{d}/verify.out' 2>&1\necho $? > '{d}/verify.rc'\n",
            d = dir.display(),
            s = state.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&rt, std::fs::Permissions::from_mode(0o755)).unwrap();
    rt
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// One run of the unit in the headless config dir. `model`: with `--runtime`.
fn unit(h: &Hub, state: &Path, model: Option<(&Path, &Path)>) -> Out {
    let mut args = vec![
        "wake",
        "--tick",
        "--state-dir",
        state.to_str().unwrap(),
        "--retry-delays",
        "",
    ];
    if let Some((rt, wd)) = model {
        args.extend([
            "--initiative",
            "fake",
            "--runtime",
            "codex",
            "--runtime-bin",
            rt.to_str().unwrap(),
            "--work-dir",
            wd.to_str().unwrap(),
        ]);
    }
    h.headless(&args)
}

/// The log without the time of each line.
fn log_lines(state: &Path) -> Vec<String> {
    read(&state.join("wake.log"))
        .lines()
        .map(|l| l.split_once(' ').map_or(l, |(_, rest)| rest).to_string())
        .collect()
}

fn files_of(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn the_model_wake_runs_a_verify_under_the_scope_and_the_verdict_is_stamped() {
    settle_day();
    let h = hub();
    h.set_authority_extra(VERIFY);
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("state");
    let wd = tmp.path().join("wd");
    std::fs::create_dir_all(&wd).unwrap();
    let rt = runtime(tmp.path(), &state);

    let before = h.board.comments_of(TICKET).len();
    let heartbeat = || h.board.item_field(&h.coord.item_id, "Heartbeat");
    let heartbeat_before = heartbeat();
    let o = unit(&h, &state, Some((&rt, &wd)));
    assert_eq!((o.code, o.stdout.as_str()), (0, ""), "{}", o.all());
    // one start, for the judgment list
    assert_eq!(read(&tmp.path().join("starts")), "judgment\n");
    // the judgment file had the ticket and words of crewboard only
    let seen = read(&tmp.path().join("judgment-seen"));
    let lines: Vec<&str> = seen.lines().collect();
    assert!(
        lines[0].starts_with("judgment list, written by crewboard wake at "),
        "{seen}"
    );
    assert_eq!(
        lines[1..],
        [
            "mode: verify".to_string(),
            "verdicts today: 0/10".to_string(),
            format!("needs judgment: {}, prose Gate", key(TICKET))
        ],
        "{seen}"
    );
    // the verify of the model passed the scope, and its message has the stamp
    assert_eq!(
        read(&tmp.path().join("verify.rc")).trim(),
        "0",
        "{}",
        read(&tmp.path().join("verify.out"))
    );
    assert!(read(&tmp.path().join("verify.out")).starts_with("verify PASS"));
    let comments = h.board.comments_of(TICKET);
    assert_eq!(comments.len(), before + 1, "one message: the verdict");
    let (_, _, body) = comments.last().unwrap();
    assert_eq!(
        body.lines()
            .filter(|l| l.starts_with("unattended:"))
            .collect::<Vec<_>>(),
        [unattended_line(1, 10)],
        "{body}"
    );
    assert!(body.contains("I read the diff: the demo runs"), "{body}");
    // the count of the day, written before the start
    let wakes = read(&state.join("coordinator-wakes"));
    let words: Vec<&str> = wakes.split_whitespace().collect();
    assert_eq!(
        (words[1], words[4], words[5]),
        ("1", "verify", "codex"),
        "{wakes}"
    );
    let log = read(&state.join("wake.log"));
    assert!(
        log.contains("1 ticket(s) need a judgment; waking the model (wake 1 of 6 today): cd "),
        "{log}"
    );
    assert!(log.contains("model wake finished OK"), "{log}");
    assert!(!state.join("wake.lock").exists());

    // no heartbeat of the coordinator: the owner alert for a late interactive coordinator
    // reads that field
    assert_eq!(heartbeat(), heartbeat_before);

    // the next run: the ticket has its verdict, the list is empty, no model starts
    let o = unit(&h, &state, Some((&rt, &wd)));
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(read(&tmp.path().join("starts")), "judgment\n");
    assert_eq!(h.board.comments_of(TICKET).len(), before + 1);
}

#[test]
fn with_the_key_off_the_unit_with_a_runtime_does_what_a_tick_unit_does_and_starts_nothing() {
    for extra in ["", "unattended_coordinator: off\n"] {
        let tmp = tempfile::tempdir().unwrap();
        let wd = tmp.path().join("wd");
        std::fs::create_dir_all(&wd).unwrap();
        // two boards with the same ticket: a plain tick unit, and a unit with a runtime
        let (plain, model) = (hub(), hub());
        plain.set_authority_extra(extra);
        model.set_authority_extra(extra);
        let (state_a, state_b) = (tmp.path().join("a"), tmp.path().join("b"));
        let rt = runtime(tmp.path(), &state_b);
        for _ in 0..2 {
            let a = unit(&plain, &state_a, None);
            let b = unit(&model, &state_b, Some((&rt, &wd)));
            assert_eq!((a.code, b.code), (0, 0), "{} {}", a.all(), b.all());
            assert_eq!((a.stdout.as_str(), b.stdout.as_str()), ("", ""));
            assert_eq!((a.stderr.as_str(), b.stderr.as_str()), ("", ""));
        }
        assert_eq!(log_lines(&state_a), log_lines(&state_b), "{extra:?}");
        assert!(!log_lines(&state_a).is_empty());
        assert_eq!(files_of(&state_a), files_of(&state_b), "{extra:?}");
        assert!(!tmp.path().join("starts").exists(), "no process started");
        // the same requests, and no write
        let reqs = |h: &Hub| -> Vec<String> {
            h.requests()
                .iter()
                .map(|r| format!("{} {}", r.method, r.path.split('?').next().unwrap_or("")))
                .collect()
        };
        assert_eq!(reqs(&plain), reqs(&model), "{extra:?}");
        assert!(model.writes().is_empty());
    }
}

#[test]
fn a_judgment_that_the_scope_refuses_is_refused_inside_the_model_wake_too() {
    settle_day();
    let h = hub();
    h.set_authority_extra(VERIFY);
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("state");
    let wd = tmp.path().join("wd");
    std::fs::create_dir_all(&wd).unwrap();
    // a runtime that asks for a merge and a reassign: both are outside the scope
    use std::os::unix::fs::PermissionsExt;
    let rt = tmp.path().join("runtime");
    let bin = env!("CARGO_BIN_EXE_crewboard");
    std::fs::write(
        &rt,
        format!(
            "#!/bin/sh\n'{bin}' merge {TICKET} > '{d}/merge.out' 2>&1\necho $? > '{d}/merge.rc'\n'{bin}' reassign {TICKET} --reason late --why x > '{d}/reassign.out' 2>&1\necho $? > '{d}/reassign.rc'\n",
            d = tmp.path().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&rt, std::fs::Permissions::from_mode(0o755)).unwrap();
    let before = h.board.comments_of(TICKET).len();
    let o = unit(&h, &state, Some((&rt, &wd)));
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(read(&tmp.path().join("merge.rc")).trim(), "2");
    assert!(read(&tmp.path().join("merge.out")).contains("[rule=headless-scope]"));
    assert_eq!(read(&tmp.path().join("reassign.rc")).trim(), "2");
    assert!(read(&tmp.path().join("reassign.out")).contains("[rule=headless-scope]"));
    assert_eq!(h.board.comments_of(TICKET).len(), before);
    assert!(!h.merged());
}
