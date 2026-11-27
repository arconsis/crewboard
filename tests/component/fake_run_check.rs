//! `verify --run-check` (spec/ci#checks-that-the-tool-runs), against the fake GitHub and a real
//! git repo: the tool makes a clean checkout of the commit of the `done`, runs the named checks
//! of AUTHORITY.md without a shell, and writes the local-run line of the verify itself.

use crate::support::authority::{NEEDS_OWNER_ALL, authority_with};
use crate::support::board::{Agent, FakeBoard, Out, REPO};
use crate::support::cli::writes;
use crewboard::rules::Role;
use std::path::{Path, PathBuf};
use std::process::Command;

const CHECKS: &str = "ci_mode: local\ncheck_env: [IT_WORDS]\ncheck_timeout: 2\nchecks:\n  - slow: sleep 30\n  - missing: no-such-program-of-this-test\n  - env: sh -c 'test \"$IT_WORDS\" = abc && test -z \"$IT_OTHER\"'\n  - marker: sh -c 'test -f marker && echo ok-from-check'\n  - clean: sh -c 'test ! -e untracked && test -z \"$GH_TOKEN\"'\n  - script: ./check.sh one\n  - red: sh -c 'echo boom; exit 3'\n";

fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

struct Case {
    fake: FakeBoard,
    c: Agent,
    ts: String,
    /// The checkout of the coordinator; `origin` is a repo whose path ends with the repo name.
    work: PathBuf,
    state: PathBuf,
    sha: String,
    /// The number of the PR.
    pr: u64,
    _tmp: tempfile::TempDir,
}

/// A ticket in Review with a local-run `done` whose head is a real commit. The checkout of the
/// coordinator has an untracked file, which a clean checkout must not have.
fn case() -> Case {
    case_gate("the checks pass")
}

/// [`case`] with the Gate line of the ticket.
fn case_gate(gate: &str) -> Case {
    let (k, b, url) = case_taken(gate);
    let o = k.fake.run(
        &b.dir,
        &[
            "done",
            &k.ts,
            "--pr",
            &url,
            "--sha",
            &k.sha,
            "--local-run",
            "host-b: ./check.sh one rc=0, log /tmp/x.log",
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    k
}

/// A ticket In Progress with an open PR whose head is a real commit, before any `done`: the
/// case, the dev agent and the URL of the PR.
fn case_taken(gate: &str) -> (Case, Agent, String) {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin").join(REPO);
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q", "-b", "main"]);
    std::fs::write(origin.join("marker"), "x\n").unwrap();
    std::fs::write(
        origin.join("check.sh"),
        "#!/bin/sh\ntest \"$1\" = one && test -f marker && echo script-ran-in \"$(pwd)\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(
        origin.join("check.sh"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    git(&origin, &["add", "."]);
    git(&origin, &["commit", "-q", "-m", "work"]);
    let sha = git(&origin, &["rev-parse", "HEAD"]);
    let work = tmp.path().join("work");
    git(
        tmp.path(),
        &["clone", "-q", &origin.to_string_lossy(), "work"],
    );
    std::fs::write(work.join("untracked"), "left by a person\n").unwrap();

    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    fake.file(
        "AUTHORITY.md",
        &authority_with("fake-owner", NEEDS_OWNER_ALL, CHECKS),
    );
    let t = fake.ticket_issue(
        "IT run check",
        &format!("Goal: x.\n\nGate: {gate}\n"),
        &["crew:work"],
    );
    let ts = t.to_string();
    assert_eq!(fake.run(&c.dir, &["assign", &ts, "--to", &b.name]).code, 0);
    assert_eq!(fake.run(&b.dir, &["take", &ts]).code, 0);
    let pr = fake.pr_for(t, &b.name);
    fake.set_pr_head(pr.number, &sha);
    let state = tmp.path().join("state");
    let url = pr.url.clone();
    (
        Case {
            fake,
            c,
            ts,
            work,
            state,
            sha,
            pr: pr.number,
            _tmp: tmp,
        },
        b,
        url,
    )
}

fn verify(k: &Case, extra: &[&str]) -> Out {
    let mut args = vec!["verify", k.ts.as_str()];
    args.extend_from_slice(extra);
    k.fake.run_env(
        &k.c.dir,
        &k.work,
        &args,
        &[
            ("XDG_STATE_HOME", &k.state.to_string_lossy()),
            ("IT_WORDS", "abc"),
            ("IT_OTHER", "not listed"),
        ],
    )
}

fn work_trees(k: &Case) -> usize {
    git(&k.work, &["worktree", "list"]).lines().count()
}

#[test]
fn the_tool_runs_the_checks_on_a_clean_checkout_and_writes_the_line_itself() {
    let k = case();
    let o = verify(
        &k,
        &[
            "--run-check",
            "marker",
            "--run-check",
            "clean",
            "--run-check",
            "script",
            // a variable that check_env names reaches the check; another one does not
            "--run-check",
            "env",
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    // the signed verify has the line of the tool, with each check and the commit
    let body = k
        .fake
        .comments_of(k.ts.parse().unwrap())
        .into_iter()
        .map(|(_, _, b)| b)
        .find(|b| b.contains("coordinator-local-gate:"))
        .unwrap_or_else(|| panic!("no verify with a gate line: {}", o.all()));
    let line = body
        .lines()
        .find(|l| l.starts_with("coordinator-local-gate:"))
        .unwrap();
    assert!(
        line.starts_with("coordinator-local-gate: tool-run fake-c: marker rc=0 "),
        "{line}"
    );
    assert!(
        line.contains("clean rc=0") && line.contains("script rc=0"),
        "{line}"
    );
    assert!(
        line.contains(&format!("clean checkout of {}", &k.sha[..12])),
        "{line}"
    );
    // the logs stay, the work tree is gone, and the checkout of the coordinator is as it was
    let runs = k.state.join("crewboard").join("check-runs");
    let run = std::fs::read_dir(&runs)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let log = std::fs::read_to_string(run.join("marker.log")).unwrap();
    assert_eq!(log.trim(), "ok-from-check");
    let script = std::fs::read_to_string(run.join("script.log")).unwrap();
    assert!(
        script.contains("script-ran-in") && script.contains("tree"),
        "{script}"
    );
    assert!(!run.join("tree").exists());
    assert_eq!(work_trees(&k), 1);
    assert!(k.work.join("untracked").exists());
}

#[test]
fn a_check_that_fails_is_refused_with_its_log_and_writes_nothing() {
    let k = case();
    let before = writes(&k.fake);
    let o = verify(&k, &["--run-check", "marker", "--run-check", "red"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("rule=check-failed"), "{}", o.all());
    assert!(o.stderr.contains("check red exited 3"), "{}", o.all());
    assert!(o.stderr.contains("| boom"), "{}", o.all());
    assert!(
        o.stderr.contains("verify") && o.stderr.contains("--fail"),
        "{}",
        o.all()
    );
    assert_eq!(writes(&k.fake), before, "{}", o.all());
    assert_eq!(work_trees(&k), 1);
}

#[test]
fn an_unknown_check_a_typed_tool_line_and_a_foreign_checkout_are_refused_before_any_run() {
    let k = case();
    let before = writes(&k.fake);
    let o = verify(&k, &["--run-check", "tests"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("rule=check-unknown"), "{}", o.all());
    assert!(
        o.stderr
            .contains("AUTHORITY.md has: slow, missing, env, marker, clean, script, red"),
        "{}",
        o.all()
    );
    // a model cannot type the line of the tool: not as the text, not in another spelling, and
    // not as a line of the note
    for args in [
        &["--local-run", "tool-run fake-c: marker rc=0"][..],
        &["--local-run", "Tool-Run fake-c: marker rc=0"],
        &["--local-run", "[tool-run] fake-c: marker rc=0"],
        &[
            "--local-run",
            "fake-c: looked at it rc=0",
            "--body",
            "ok\ncoordinator-local-gate: tool-run fake-c: marker rc=0",
        ],
    ] {
        let o = verify(&k, args);
        assert_eq!(o.code, 2, "{args:?}: {}", o.all());
        assert!(o.stderr.contains("rule=local-gate-invalid"), "{}", o.all());
    }
    // --run-check with a typed line or with --fail is a usage error
    let o = verify(&k, &["--run-check", "marker", "--local-run", "x: y rc=0"]);
    assert_eq!(o.code, 64, "{}", o.all());
    // a checkout of another repo
    let other = k.work.parent().unwrap().join("other");
    std::fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q"]);
    git(
        &other,
        &[
            "remote",
            "add",
            "origin",
            "https://example.org/some/other.git",
        ],
    );
    let o = verify(
        &k,
        &[
            "--run-check",
            "marker",
            "--work-dir",
            &other.to_string_lossy(),
        ],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("rule=check-not-run"), "{}", o.all());
    assert!(o.stderr.contains("another repo"), "{}", o.all());
    assert_eq!(writes(&k.fake), before);
    assert!(!k.state.join("crewboard").join("check-runs").exists());
}

#[test]
fn no_check_runs_for_a_writer_that_may_not_verify_or_for_a_protected_path() {
    let k = case();
    let runs = k.state.join("crewboard").join("check-runs");
    // a dev agent: refused by the rule of verify, before any code of the PR runs
    let b = k.fake.agent("hermes-2@fake-d", Role::Dev);
    let o = k.fake.run_env(
        &b.dir,
        &k.work,
        &["verify", &k.ts, "--run-check", "marker"],
        &[("XDG_STATE_HOME", &k.state.to_string_lossy())],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(!runs.exists(), "{}", o.all());
    // a PR that changes a protected path: the owner merges it; no check runs
    k.fake.set_pr_files(k.pr, &[".github/workflows/ci.yml"]);
    let o = verify(&k, &["--run-check", "marker"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("rule=protected-path-owner-merge"),
        "{}",
        o.all()
    );
    assert!(!runs.exists(), "{}", o.all());
}

#[test]
fn a_head_that_is_not_the_commit_of_the_done_runs_nothing_and_fails_the_verify() {
    let k = case();
    k.fake
        .set_pr_head(k.pr, "1111111111111111111111111111111111111111");
    let o = verify(&k, &["--run-check", "marker"]);
    // the failure of every verify for a moved head: a signed fail, and no run
    assert!(
        !k.state.join("crewboard").join("check-runs").exists(),
        "{}",
        o.all()
    );
    let all = o.all();
    assert!(all.contains("head") && !all.contains("tool-run"), "{all}");
    let bodies: Vec<String> = k
        .fake
        .comments_of(k.ts.parse().unwrap())
        .into_iter()
        .map(|(_, _, b)| b)
        .collect();
    assert!(!bodies.iter().any(|b| b.contains("tool-run")), "{bodies:?}");
}

/// `tick` of a headless copy of the coordinator, with or without a checkout.
fn tick(k: &Case, headless: &Path, with_checkout: bool) -> Out {
    let work = k.work.to_string_lossy().into_owned();
    let mut args = vec!["tick"];
    if with_checkout {
        args.extend_from_slice(&["--work-dir", &work]);
    }
    k.fake.run_env(
        headless,
        k.work.parent().unwrap(),
        &args,
        &[
            ("XDG_STATE_HOME", &k.state.to_string_lossy()),
            ("IT_WORDS", "abc"),
        ],
    )
}

fn headless_of(k: &Case) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    crate::support::headless::copy_headless(&k.c, d.path());
    d
}

fn gate_lines(k: &Case) -> Vec<String> {
    k.fake
        .comments_of(k.ts.parse().unwrap())
        .into_iter()
        .map(|(_, _, b)| b)
        .filter(|b| b.contains("coordinator-local-gate:"))
        .collect()
}

#[test]
fn the_headless_tick_runs_the_checks_of_a_typed_gate_and_verifies_but_never_merges() {
    let k = case_gate("checks: marker, clean");
    let h = headless_of(&k);
    // without a checkout the ticket is left, and nothing runs
    let before = writes(&k.fake);
    let o = tick(&k, h.path(), false);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains("no checkout to run them in"),
        "{}",
        o.all()
    );
    assert_eq!(writes(&k.fake), before);
    // with a checkout: the tool runs both checks and writes the verify
    let o = tick(&k, h.path(), true);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    let lines = gate_lines(&k);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].contains("coordinator-local-gate: tool-run fake-c: marker rc=0")
            && lines[0].contains("clean rc=0"),
        "{}",
        lines[0]
    );
    // no merge: a person merges a ticket with a checks Gate
    let merges = k
        .fake
        .requests()
        .into_iter()
        .filter(|r| r.method == "PUT" && r.path.ends_with("/merge"))
        .count();
    assert_eq!(merges, 0);
    // the next tick runs nothing again
    let before = writes(&k.fake);
    let o = tick(&k, h.path(), true);
    assert!(
        o.stdout.contains("its checks passed (verified)"),
        "{}",
        o.all()
    );
    assert_eq!(writes(&k.fake), before);
    assert_eq!(work_trees(&k), 1);
}

#[test]
fn a_failed_check_of_a_typed_gate_is_a_signed_fail_without_the_output_of_the_pr() {
    let k = case_gate("checks: marker, red");
    let h = headless_of(&k);
    let o = tick(&k, h.path(), true);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    let bodies: Vec<String> = k
        .fake
        .comments_of(k.ts.parse().unwrap())
        .into_iter()
        .map(|(_, _, b)| b)
        .collect();
    let fail = bodies
        .iter()
        .find(|b| {
            b.contains(
                "the tool ran the checks of the Gate: check red exited 3 on a clean checkout of",
            )
        })
        .unwrap_or_else(|| panic!("no fail with the fact: {bodies:?}"));
    // the output of the check stays in the log: it is text of the PR
    assert!(!fail.contains("boom"), "{fail}");
    assert!(!bodies.iter().any(|b| b.contains("tool-run")), "{bodies:?}");
    // the next tick does not run the checks again for the same done
    let runs = k.state.join("crewboard").join("check-runs");
    let n = std::fs::read_dir(&runs).unwrap().count();
    let o = tick(&k, h.path(), true);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(std::fs::read_dir(&runs).unwrap().count(), n, "{}", o.all());
}

#[test]
fn a_gate_with_one_more_word_is_prose_and_the_headless_tick_runs_nothing() {
    let k = case_gate("checks: marker and the page is unchanged");
    let h = headless_of(&k);
    let before = writes(&k.fake);
    let o = tick(&k, h.path(), true);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains("left for the interactive coordinator"),
        "{}",
        o.all()
    );
    assert_eq!(writes(&k.fake), before);
    assert!(!k.state.join("crewboard").join("check-runs").exists());
}

#[test]
fn a_check_that_runs_too_long_fails_and_one_that_cannot_start_is_not_a_fail() {
    let k = case();
    let before = writes(&k.fake);
    // the limit of AUTHORITY.md is 2 seconds: the check is stopped, and that is a failed check
    let o = verify(&k, &["--run-check", "slow"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("rule=check-failed"), "{}", o.all());
    assert!(
        o.stderr
            .contains("check slow ran longer than its limit of 2s"),
        "{}",
        o.all()
    );
    // a program that is not on the host is a fault of the host, not of the dev
    let o = verify(&k, &["--run-check", "missing"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("rule=check-not-run"), "{}", o.all());
    assert!(
        o.stderr.contains("cannot start on this host"),
        "{}",
        o.all()
    );
    assert_eq!(writes(&k.fake), before);
    assert_eq!(work_trees(&k), 1);
}

#[test]
fn a_merged_pr_gets_no_pass_that_claims_a_run() {
    let k = case();
    k.fake.merge_pr(k.pr);
    let before = writes(&k.fake);
    let o = verify(&k, &["--run-check", "marker"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("rule=check-not-run"), "{}", o.all());
    assert!(o.stderr.contains("closed or merged"), "{}", o.all());
    assert_eq!(writes(&k.fake), before);
    assert!(!k.state.join("crewboard").join("check-runs").exists());
}

#[test]
fn the_headless_tick_runs_no_check_whose_file_a_pr_can_change_and_no_program_that_is_missing() {
    // `script` runs ./check.sh, which no protected path covers
    let k = case_gate("checks: script");
    let h = headless_of(&k);
    let before = writes(&k.fake);
    let o = tick(&k, h.path(), true);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains("rule=check-not-run") || o.stdout.contains("check-not-run"),
        "{}",
        o.all()
    );
    assert!(
        o.stdout.contains("an unattended run needs every file"),
        "{}",
        o.all()
    );
    assert_eq!(writes(&k.fake), before, "{}", o.all());
    // a missing program sends nothing back to the dev, and the next tick tries again
    let k = case_gate("checks: missing");
    let h = headless_of(&k);
    let before = writes(&k.fake);
    for _ in 0..2 {
        let o = tick(&k, h.path(), true);
        assert_eq!(o.code, 0, "{}", o.all());
        assert!(
            o.stdout.contains("cannot start on this host"),
            "{}",
            o.all()
        );
    }
    assert_eq!(writes(&k.fake), before);
}

fn check(k: &Case, dir: &Path, args: &[&str]) -> Out {
    let mut all = vec!["check"];
    all.extend_from_slice(args);
    k.fake.run_env(
        dir,
        &k.work,
        &all,
        &[
            ("XDG_STATE_HOME", &k.state.to_string_lossy()),
            ("IT_WORDS", "abc"),
            ("IT_OTHER", "not listed"),
        ],
    )
}

/// `crewboard check` (spec/ci#checks-before-a-delivery): the run of `verify --run-check` on a
/// commit of the caller, without a ticket and without a write.
#[test]
fn check_runs_the_named_checks_on_a_clean_checkout_of_head_and_writes_nothing() {
    let k = case();
    let n = k.ts.parse().unwrap();
    let before = k.fake.comments_of(n).len();
    let w = writes(&k.fake);
    {
        let o = check(&k, &k.c.dir, &["marker", "clean", "script", "env"]);
        assert_eq!(o.code, 0, "{}", o.all());
        assert!(
            o.stdout.contains("ok    marker: rc=0 ")
                && o.stdout.contains("ok    clean: rc=0 ")
                && o.stdout.contains("ok    script: rc=0 ")
                && o.stdout.contains("ok    env: rc=0 "),
            "{}",
            o.all()
        );
        let last = o.stdout.lines().last().unwrap();
        assert!(
            last.starts_with(&format!(
                "4 checks passed on a clean checkout of {}. Checks from AUTHORITY.md on the default branch of ",
                &k.sha[..12]
            )) && last.ends_with("Nothing was written"),
            "{last}"
        );
        // the checkout of the caller has a file that no commit holds: the note says so
        assert!(
            o.stderr.contains("has changes that no commit holds"),
            "{}",
            o.all()
        );
    }
    assert_eq!(writes(&k.fake), w, "check wrote to GitHub");
    assert_eq!(k.fake.comments_of(n).len(), before);
    assert_eq!(work_trees(&k), 1, "the work tree of the run is removed");
}

#[test]
fn check_without_a_name_runs_every_check_in_order_and_stops_at_the_first_that_fails() {
    let k = case();
    // the first check of the file is `slow`, and the limit of the file is 2 seconds
    let o = check(&k, &k.c.dir, &[]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("rule=check-failed"), "{}", o.all());
    assert!(
        o.stderr
            .contains("check slow ran longer than its limit of 2s"),
        "{}",
        o.all()
    );
    // no later check ran, and the text names no ticket to send back
    assert!(!o.stderr.contains("check missing:"), "{}", o.all());
    assert!(!o.stderr.contains("crewboard verify"), "{}", o.all());
    assert_eq!(work_trees(&k), 1);

    let o = check(&k, &k.c.dir, &["red"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("check red exited 3") && o.stderr.contains("| boom"),
        "{}",
        o.all()
    );
    let o = check(&k, &k.c.dir, &["nope"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("rule=check-unknown"), "{}", o.all());
}

#[test]
fn check_tries_the_checks_of_a_draft_file_without_a_board_and_takes_a_rev() {
    let k = case();
    // a second commit without the marker: `--rev` picks the commit that is tested
    std::fs::remove_file(k.work.join("marker")).unwrap();
    git(&k.work, &["commit", "-q", "-am", "no marker"]);
    let draft = k.work.join("draft.md");
    std::fs::write(
        &draft,
        "---\nowner: fake-owner\nchecks:\n  - has: sh -c 'test -f marker'\n  - words: sh -c 'test -z \"$IT_WORDS\"'\ncheck_env: [IT_NOT_SET]\n---\n",
    )
    .unwrap();
    // a config dir without any board config: the file is the only source
    let empty = k.work.parent().unwrap().join("no-config");
    std::fs::create_dir_all(&empty).unwrap();
    let file = draft.to_string_lossy().into_owned();
    let requests = k.fake.requests().len();
    {
        let o = check(&k, &empty, &["--authority-file", &file, "--rev", &k.sha]);
        assert_eq!(o.code, 0, "{}", o.all());
        assert!(
            o.stdout.contains(&format!(
                "2 checks passed on a clean checkout of {}. Checks from the file ",
                &k.sha[..12]
            )),
            "{}",
            o.all()
        );
        // a name of check_env that this process does not have is named
        assert!(
            o.stderr.contains("check_env names IT_NOT_SET"),
            "{}",
            o.all()
        );
    }
    assert_eq!(
        k.fake.requests().len(),
        requests,
        "a draft file needs no board"
    );
    // HEAD has no marker
    let o = check(&k, &empty, &["--authority-file", &file, "has"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("check has exited 1"), "{}", o.all());
    // a rev that names no commit is a usage error, and nothing runs
    let o = check(
        &k,
        &empty,
        &["--authority-file", &file, "--rev", "no-such-rev"],
    );
    assert_eq!(o.code, 64, "{}", o.all());
    assert!(
        o.stderr.contains("no-such-rev names no commit"),
        "{}",
        o.all()
    );
    // a file without front matter has no checks
    std::fs::write(&draft, "no front matter\n").unwrap();
    let o = check(&k, &empty, &["--authority-file", &file]);
    assert_ne!(o.code, 0, "{}", o.all());
    assert!(o.stderr.contains("has no front matter"), "{}", o.all());
}

#[test]
fn check_under_a_wake_runs_no_check_whose_file_a_pr_can_change() {
    let k = case();
    let state = k.state.to_string_lossy().into_owned();
    // `script` runs ./check.sh, which no protected path covers
    let o = k.fake.run_env(
        &k.c.dir,
        &k.work,
        &["check", "script"],
        &[("XDG_STATE_HOME", &state), ("CREWBOARD_WAKE", "1")],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("rule=check-not-run"), "{}", o.all());
    assert!(!o.stdout.contains("ok    script"), "{}", o.all());
    // a check without a file of the repo runs
    let o = k.fake.run_env(
        &k.c.dir,
        &k.work,
        &["check", "marker"],
        &[("XDG_STATE_HOME", &state), ("CREWBOARD_WAKE", "1")],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(work_trees(&k), 1);
}

fn done_run(k: &Case, b: &Agent, url: &str, extra: &[&str]) -> Out {
    let mut args = vec!["done", k.ts.as_str(), "--pr", url];
    args.extend_from_slice(extra);
    k.fake.run_env(
        &b.dir,
        &k.work,
        &args,
        &[
            ("XDG_STATE_HOME", &k.state.to_string_lossy()),
            ("IT_WORDS", "abc"),
        ],
    )
}

/// `done --run-check` (spec/ci#checks-in-a-delivery): the tool runs the checks on a clean
/// checkout of the head of the PR and writes the local-run line of the delivery itself.
#[test]
fn done_runs_the_checks_and_writes_the_line_of_the_delivery_itself() {
    let (k, b, url) = case_taken("the checks pass");
    let o = done_run(
        &k,
        &b,
        &url,
        &["--run-check", "marker", "--run-check", "clean"],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    let body = k
        .fake
        .comments_of(k.ts.parse().unwrap())
        .into_iter()
        .map(|(_, _, b)| b)
        .find(|b| b.lines().any(|l| l.starts_with("local-gate:")))
        .unwrap_or_else(|| panic!("no done with a gate line: {}", o.all()));
    let line = body.lines().find(|l| l.starts_with("local-gate:")).unwrap();
    assert!(
        line.starts_with("local-gate: tool-run fake-b: marker rc=0 ")
            && line.contains("clean rc=0"),
        "{line}"
    );
    assert!(
        line.contains(&format!("clean checkout of {}", &k.sha[..12])),
        "{line}"
    );
    assert_eq!(work_trees(&k), 1, "the work tree of the run is removed");
    // the coordinator verifies this delivery as every local-run delivery
    let o = verify(&k, &["--run-check", "marker"]);
    assert_eq!(o.code, 0, "{}", o.all());
}

#[test]
fn a_done_whose_check_fails_writes_nothing_and_the_ticket_stays_in_progress() {
    let (k, b, url) = case_taken("the checks pass");
    let n = k.ts.parse().unwrap();
    let before = k.fake.comments_of(n).len();
    let w = writes(&k.fake);
    let o = done_run(&k, &b, &url, &["--run-check", "red"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("rule=check-failed"), "{}", o.all());
    assert!(
        o.stderr.contains("check red exited 3") && o.stderr.contains("run done again"),
        "{}",
        o.all()
    );
    // an unknown name, a commit that is not the head, and both ways at once: no run, no write
    let o = done_run(&k, &b, &url, &["--run-check", "nope"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("rule=check-unknown"), "{}", o.all());
    let other = "0123456789abcdef0123456789abcdef01234567";
    let o = done_run(&k, &b, &url, &["--sha", other, "--run-check", "marker"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("rule=evidence-head-moved"), "{}", o.all());
    assert!(!o.stderr.contains("check marker: running"), "{}", o.all());
    let o = done_run(
        &k,
        &b,
        &url,
        &[
            "--run-check",
            "marker",
            "--local-run",
            "host-b: x rc=0, log /tmp/x",
        ],
    );
    assert_eq!(o.code, 64, "{}", o.all());
    assert_eq!(writes(&k.fake), w, "a refused done wrote to GitHub");
    assert_eq!(k.fake.comments_of(n).len(), before);
    assert_eq!(work_trees(&k), 1);
}

/// No code of a PR runs before `done` knows that the PR is the PR of this ticket and this
/// agent, and a second `done` of the same head costs no run.
#[test]
fn done_runs_no_check_for_a_pr_that_it_refuses_anyway() {
    let (k, b, url) = case_taken("the checks pass");
    // a PR of another branch whose head is a commit that the checks would pass
    let other = k
        .fake
        .pr_on_branch("someone/else", &format!("Closes #{}", k.ts));
    k.fake.set_pr_head(other.number, &k.sha);
    let o = done_run(&k, &b, &other.url, &["--run-check", "marker"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("rule=evidence-branch-name"),
        "{}",
        o.all()
    );
    assert!(!o.stderr.contains("check marker: running"), "{}", o.all());

    // the first done lands; the same done again is `status-no-change`, without a run
    let o = done_run(&k, &b, &url, &["--run-check", "marker"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let o = done_run(&k, &b, &url, &["--run-check", "marker"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("rule=status-no-change"), "{}", o.all());
    assert!(!o.stderr.contains("check marker: running"), "{}", o.all());
    assert_eq!(work_trees(&k), 1);
}
