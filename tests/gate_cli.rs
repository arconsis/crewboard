//! `crewboard repeat` (old name `gate run`) as the real binary: the repeated-run gate. The cases carry the
//! ids of the shell suite (`skill/loops/tests/test-locks.sh`, G1 to G10e) that this file replaces
//! as the proof of the logic. `HOST_LOCK_PS_CMD` plays the process list of the CI check, so a test
//! never sees the real CI of the machine it runs on.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::disallowed_methods,
    reason = "a test fails by panic and waits for real processes"
)]

use std::process::Command;

#[cfg(unix)]
#[path = "support/lock.rs"]
mod lock;

const BIN: &str = env!("CARGO_BIN_EXE_crewboard");

/// The platform rule: `--lock` needs a Unix host. On Windows the gate refuses it with the rule
/// `unix-only` (exit 2) before it touches a file, and it does not refuse without `--lock`; on
/// Unix it does not refuse at all.
#[test]
fn lock_is_refused_with_unix_only_on_windows_only() {
    let d = tempfile::tempdir().unwrap();
    let missing = d.path().join("no-such-repo");
    let gate = |extra: &[&str]| {
        let o = Command::new(BIN)
            .args(["repeat", "--times", "1", "--no-ci-check"])
            .args(extra)
            .arg("--work-dir")
            .arg(&missing)
            .args(["--", "true"])
            .output()
            .unwrap();
        (
            o.status.code(),
            String::from_utf8_lossy(&o.stderr).to_string(),
        )
    };
    let (code, err) = gate(&["--lock", d.path().join("l").to_str().unwrap()]);
    if cfg!(windows) {
        assert_eq!(code, Some(2), "{err}");
        assert!(err.contains("rule=unix-only"), "{err}");
    } else {
        // no refusal: the run goes on and stops at the missing repo (exit 4)
        assert_eq!(code, Some(4), "{err}");
        assert!(!err.contains("unix-only"), "{err}");
    }
    // without --lock the gate works on every platform: it stops at the missing repo (exit 4)
    let (code, err) = gate(&[]);
    assert_eq!(code, Some(4), "{err}");
    assert!(err.contains("--work-dir is not a directory"), "{err}");
}

#[cfg(unix)]
mod unix {
    use super::BIN;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use super::lock::{self, Out, Rig, out_of};

    const GREEN: &str = "echo 'test result: ok. 5 passed; 0 failed; 0 ignored'";
    const CARGO_RE: &str = "^test result: ";

    /// A git checkout with one commit, and the rig of the lock tests (a live pid, a fake process
    /// list).
    struct Gate {
        rig: Rig,
        repo: PathBuf,
    }

    fn git(dir: &Path, args: &[&str]) {
        let st = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.email=t@x", "-c", "user.name=t"])
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?}");
    }

    impl Gate {
        fn new() -> Gate {
            let rig = Rig::new();
            let repo = rig.p("repo");
            std::fs::create_dir(&repo).unwrap();
            git(&repo, &["init", "-q"]);
            git(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
            Gate { rig, repo }
        }

        fn sha(&self) -> String {
            let o = Command::new("git")
                .arg("-C")
                .arg(&self.repo)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        }

        fn logs(&self, name: &str) -> String {
            self.rig.s(name)
        }

        /// `crewboard repeat <args>` with a clean environment.
        fn cmd(&self, args: &[&str]) -> Command {
            self.cmd_as(&["repeat"], args)
        }

        /// `crewboard <head> <args>` with a clean environment (`gate run` is the old head).
        fn cmd_as(&self, head: &[&str], args: &[&str]) -> Command {
            let mut c = Command::new(BIN);
            c.args(head)
                .args(args)
                .env("HOST_LOCK_HOST", lock::HOST)
                .env("HOST_LOCK_PS_CMD", format!("cat {}", self.rig.s("ps.txt")))
                .env_remove("CREWBOARD_HOST_LOCK")
                .env_remove("CREWBOARD_HOST_LOCK_PID")
                .env_remove("CREWBOARD_CONFIG_DIR")
                .env_remove("CREWBOARD_AGENT")
                .env_remove("CREWBOARD_WAKE")
                .env_remove("HOST_LOCK_MAX_AGE")
                .env_remove("HOST_LOCK_EMPTY_GRACE")
                .env_remove("HOST_LOCK_BOARD_TTL")
                .env_remove("HOST_LOCK_PARENT_CMD")
                .stdin(Stdio::null());
            c
        }

        /// Run the gate in this repo with the log dir `name`; `args` come first, then `-- cmd`.
        fn run(&self, name: &str, args: &[&str], cmd: &[&str]) -> Out {
            let logs = self.logs(name);
            let repo = self.repo.to_string_lossy().to_string();
            let mut a = vec!["--work-dir", &repo, "--log-dir", &logs];
            a.extend_from_slice(args);
            a.push("--");
            a.extend_from_slice(cmd);
            out_of(self.cmd(&a).output().unwrap())
        }

        /// The same with a shell command line as the command (the tests need a counter or a pipe).
        fn sh(&self, name: &str, args: &[&str], script: &str) -> Out {
            self.run(name, args, &["sh", "-c", script])
        }

        fn log(&self, name: &str, n: usize) -> String {
            std::fs::read_to_string(self.rig.p(name).join(format!("run-{n}.log")))
                .unwrap_or_else(|_| "<no log>".to_string())
        }
    }

    fn lines_of(text: &str, start: &str) -> usize {
        text.lines().filter(|l| l.starts_with(start)).count()
    }

    /// The warning lines of the old names: on stderr only, one per old name.
    fn warning_lines(o: &Out) -> Vec<&str> {
        o.stderr
            .lines()
            .filter(|l| l.starts_with("warning: "))
            .collect()
    }

    /// `gate run` and the old option names `--runs`, `--repo` and `--owner` are gone.
    #[test]
    fn the_old_command_and_the_old_option_names_are_gone() {
        let g = Gate::new();
        let repo = g.repo.to_string_lossy().to_string();
        let o = out_of(
            g.cmd_as(&["gate", "run"], &["--", "true"])
                .output()
                .unwrap(),
        );
        assert_ne!(o.code, 0, "{}", o.all());
        assert!(o.stderr.contains("unrecognized subcommand"), "{}", o.stderr);
        let logs = g.logs("old-names");
        for old in ["--runs", "--repo"] {
            let o = out_of(
                g.cmd(&[
                    old,
                    &repo,
                    "--log-dir",
                    &logs,
                    "--no-ci-check",
                    "--",
                    "true",
                ])
                .output()
                .unwrap(),
            );
            assert_eq!(o.code, 64, "{old}: {}", o.all());
        }
        let ok = out_of(
            g.cmd(&["--times", "1", "--no-ci-check", "--", "true"])
                .output()
                .unwrap(),
        );
        assert!(warning_lines(&ok).is_empty(), "{}", ok.stderr);
    }

    #[test]
    fn three_green_runs_pass() {
        // G1
        let g = Gate::new();
        let o = g.sh("g1", &["--times", "3", "--result-re", CARGO_RE], GREEN);
        assert_eq!(o.code, 0, "{}", o.all());
        assert_eq!(lines_of(&o.stdout, "  test result: ok. 5 passed"), 3);
        assert!(
            o.stdout.contains("PASS: 3 of 3 runs green on "),
            "{}",
            o.stdout
        );
        for n in 1..=3 {
            assert!(g.rig.p("g1").join(format!("run-{n}.log")).is_file());
        }
        // the same runs without a result pattern are judged by the exit code, and pass
        let o = g.sh("g4b", &["--times", "3"], GREEN);
        assert_eq!(o.code, 0, "{}", o.all());
    }

    #[test]
    fn the_report_is_the_evidence_block_between_its_header_and_the_verdict() {
        let g = Gate::new();
        let o = g.run(
            "rep",
            &["--times", "2", "--result-re", CARGO_RE],
            &["sh", "-c", GREEN],
        );
        assert_eq!(o.code, 0, "{}", o.all());
        let (sha, logs) = (g.sha(), g.logs("rep"));
        let want = format!(
            "gate-loop: evidence (cmd: sh -c 'echo '\\''test result: ok. 5 passed; 0 failed; 0 ignored'\\''')\n\
             run 1: sha={sha} exit=0 log={logs}/run-1.log\n\
             \x20 test result: ok. 5 passed; 0 failed; 0 ignored\n\
             run 2: sha={sha} exit=0 log={logs}/run-2.log\n\
             \x20 test result: ok. 5 passed; 0 failed; 0 ignored\n\
             gate-loop: PASS: 2 of 2 runs green on {sha}\n\
             gate-loop: logs in {logs}\n"
        );
        assert_eq!(o.stdout, want);
        // the log lines of the gate go to stderr, not into the report
        assert!(!o.stderr.contains("evidence"), "{}", o.stderr);
    }

    #[test]
    fn red_run_stops_the_loop() {
        // run 2 of 4 prints a failure and exits 101, there is no run 3
        let g = Gate::new();
        let count = g.rig.s("g2.count");
        let script = format!(
            "c=$(( $(cat {count} 2>/dev/null || echo 0) + 1 )); echo $c >{count}; \
             if [ $c -eq 2 ]; then echo 'test result: FAILED. 4 passed; 1 failed'; exit 101; fi; \
             echo 'test result: ok. 5 passed; 0 failed'"
        );
        let o = g.sh("g2", &["--times", "4"], &script);
        assert_eq!(o.code, 1, "{}", o.all());
        assert!(
            o.stdout
                .contains("RED at run 2 of 4: the command exited 101 (1 green runs before it)"),
            "{}",
            o.stdout
        );
        assert!(!g.rig.p("g2/run-3.log").exists());
        assert_eq!(std::fs::read_to_string(&count).unwrap().trim(), "2");
        // without --result-re a non-zero exit is red at once, whatever the log says
        let o = g.sh(
            "g10b",
            &["--times", "3"],
            "echo 'Tests: 1 failed, 4 passed, 5 total'; exit 1",
        );
        assert_eq!(o.code, 1, "{}", o.all());
        assert!(o.stdout.contains("RED at run 1 of 3: the command exited 1"));
        assert!(!g.rig.p("g10b/run-2.log").exists());
    }

    #[test]
    fn old_result_line_never_counts() {
        // an old green line in the log dir, a new red run
        let g = Gate::new();
        let ld = g.rig.p("g3");
        std::fs::create_dir(&ld).unwrap();
        std::fs::write(
            ld.join("run-1.log"),
            "test result: ok. 99 passed; 0 failed\n",
        )
        .unwrap();
        let o = g.sh(
            "g3",
            &["--times", "1", "--result-re", CARGO_RE],
            "echo 'test result: FAILED. 1 passed; 1 failed'",
        );
        assert_eq!(o.code, 1, "{}", o.all());
        assert!(!o.stdout.contains("99 passed"), "{}", o.stdout);
        // an old green line, and the new run prints no result line: red, the old line is gone
        std::fs::write(
            ld.join("run-1.log"),
            "test result: ok. 99 passed; 0 failed\n",
        )
        .unwrap();
        let o = g.sh(
            "g3",
            &["--times", "1", "--result-re", CARGO_RE],
            "echo building",
        );
        assert_eq!(o.code, 1, "{}", o.all());
        assert!(o.stdout.contains("(no result line)"), "{}", o.stdout);
        assert!(!o.stdout.contains("99 passed"), "{}", o.stdout);
        assert_eq!(g.log("g3", 1), "building\n");
    }

    #[test]
    fn sha_change_fails_exit_2() {
        // the command of run 2 commits; run 2 is not counted and there is no run 3
        let g = Gate::new();
        let count = g.rig.s("g4.count");
        let script = format!(
            "c=$(( $(cat {count} 2>/dev/null || echo 0) + 1 )); echo $c >{count}; \
             if [ $c -eq 2 ]; then git -c user.email=t@x -c user.name=t commit -q --allow-empty -m two; fi; \
             echo 'test result: ok. 5 passed; 0 failed'"
        );
        let first = g.sha();
        let o = g.sh("g4", &["--times", "3"], &script);
        assert_eq!(o.code, 2, "{}", o.all());
        let now = g.sha();
        assert_ne!(first, now);
        assert!(
            o.stdout.contains(&format!(
                "SHA CHANGED during run 2 of 3 ({first} -> {now}): run 2 is not counted"
            )),
            "{}",
            o.stdout
        );
        assert!(!o.stdout.contains("PASS"), "{}", o.stdout);
        assert!(!g.rig.p("g4/run-3.log").exists());
        // the evidence names the new commit for run 2
        assert!(
            o.stdout.contains(&format!("run 2: sha={now} exit=0")),
            "{}",
            o.stdout
        );
        // a run that starts on another commit than the first one is not run at all
        let o = g.sh("g4c", &["--times", "1"], "true");
        assert_eq!(o.code, 0, "{}", o.all());
    }

    #[test]
    fn exit_0_without_result_line_is_red() {
        // G5
        let g = Gate::new();
        let o = g.sh(
            "g5",
            &["--times", "2", "--result-re", CARGO_RE],
            "echo hello",
        );
        assert_eq!(o.code, 1, "{}", o.all());
        let log = format!("{}/run-1.log", g.logs("g5"));
        assert!(
            o.stdout.contains(&format!(
                "RED at run 1 of 2: no result line in {log} matches '{CARGO_RE}' (a missing line is not green)"
            )),
            "{}",
            o.stdout
        );
        // the other direction: with the line it is green
        let o = g.sh("g5", &["--times", "2", "--result-re", CARGO_RE], GREEN);
        assert_eq!(o.code, 0, "{}", o.all());
    }

    #[test]
    fn failed_count_is_red() {
        // exit 0 but a result line that reports a failure
        let g = Gate::new();
        let o = g.sh(
            "g6",
            &["--times", "1", "--result-re", CARGO_RE],
            "echo 'test result: ok. 3 passed; 2 failed'",
        );
        assert_eq!(o.code, 1, "{}", o.all());
        assert!(
            o.stdout
                .contains("RED at run 1 of 1: a result line of this run reports a failure"),
            "{}",
            o.stdout
        );
        // another runner with its own pattern passes on two green runs
        let re = "^== .*(passed|failed)";
        let o = g.sh(
            "g6b",
            &["--times", "2", "--result-re", re],
            "echo '== 46 passed in 1.2s =='",
        );
        assert_eq!(o.code, 0, "{}", o.all());
        // and '1 failed, 46 passed' is red
        let o = g.sh(
            "g6c",
            &["--times", "1", "--result-re", re],
            "echo '== 1 failed, 46 passed in 1.2s =='",
        );
        assert_eq!(o.code, 1, "{}", o.all());
    }

    #[test]
    fn a_fail_re_of_the_caller_replaces_the_default() {
        let g = Gate::new();
        let line = "echo 'done: 3 FAILED'";
        let o = g.sh(
            "fr1",
            &["--times", "1", "--result-re", "^done", "--fail-re", "ERROR"],
            line,
        );
        assert_eq!(o.code, 0, "{}", o.all());
        let o = g.sh("fr2", &["--times", "1", "--result-re", "^done"], line);
        assert_eq!(o.code, 1, "{}", o.all());
    }

    #[test]
    fn the_runner_examples_of_the_help_text_judge_as_documented() {
        // Jest and pytest, a failed count is red on exit 0, a clean line is green
        let g = Gate::new();
        let jest = "^Tests: ";
        let o = g.sh(
            "j1",
            &["--times", "1", "--result-re", jest],
            "echo 'Tests:       1 failed, 4 passed, 5 total'",
        );
        assert_eq!(o.code, 1, "{}", o.all());
        let o = g.sh(
            "j2",
            &["--times", "1", "--result-re", jest],
            "echo 'Tests:       5 passed, 5 total'",
        );
        assert_eq!(o.code, 0, "{}", o.all());
        assert!(
            o.stdout.contains("\n  Tests:       5 passed, 5 total\n"),
            "{}",
            o.stdout
        );
        let py = "^=+ .*(passed|failed)";
        let o = g.sh(
            "p1",
            &["--times", "1", "--result-re", py],
            "echo '============ 5 passed in 0.12s ============'",
        );
        assert_eq!(o.code, 0, "{}", o.all());
        let o = g.sh(
            "p2",
            &["--times", "1", "--result-re", py],
            "echo collecting",
        );
        assert_eq!(o.code, 1, "{}", o.all());
        // without --result-re the exit code alone decides, and the evidence says so
        let o = g.sh(
            "j3",
            &["--times", "2"],
            "echo 'Tests:       5 passed, 5 total'",
        );
        assert_eq!(o.code, 0, "{}", o.all());
        assert_eq!(
            lines_of(&o.stdout, "  (judged by the exit code; no --result-re)"),
            2
        );
    }

    #[test]
    fn the_help_shows_result_line_examples_for_several_stacks() {
        // G10e
        let o = out_of(
            Command::new(BIN)
                .args(["repeat", "--help"])
                .output()
                .unwrap(),
        );
        assert_eq!(o.code, 0, "{}", o.all());
        for want in [
            "cargo:    --result-re '^test result: '",
            "Jest:     --result-re '^Tests: '",
            "pytest:   --result-re '^=+ .*(passed|failed)'",
            "go test:  --result-re '^(ok|FAIL)\\s'",
            "Maven:    --result-re '^Tests run:'",
            "Gradle:   --result-re '^BUILD (SUCCESSFUL|FAILED)'",
            "Vitest:   --result-re '^\\s*Tests\\s'",
            "xcodebuild: --result-re '^\\*\\* TEST (SUCCEEDED|FAILED) \\*\\*'",
            "the exit code alone is enough",
        ] {
            assert!(o.stdout.contains(want), "{want} missing in:\n{}", o.stdout);
        }
    }

    #[test]
    fn the_command_runs_in_the_repo_without_a_shell_and_both_streams_go_to_the_log() {
        let g = Gate::new();
        // pwd: the command runs in --work-dir, whatever the current dir of the gate is
        let o = g.run("cwd", &["--times", "1"], &["pwd"]);
        assert_eq!(o.code, 0, "{}", o.all());
        let pwd = std::fs::canonicalize(g.log("cwd", 1).trim()).unwrap();
        assert_eq!(pwd, std::fs::canonicalize(&g.repo).unwrap());
        // no shell: a word with a ';' and a '$' reaches the program as one argument
        let o = g.run("argv", &["--times", "1"], &["echo", "a b;c", "$HOME"]);
        assert_eq!(o.code, 0, "{}", o.all());
        assert_eq!(g.log("argv", 1), "a b;c $HOME\n");
        // stdout and stderr of the command are in the log, and neither reaches the terminal
        let o = g.sh("both", &["--times", "1"], "echo to-out; echo to-err >&2");
        assert_eq!(o.code, 0, "{}", o.all());
        assert_eq!(g.log("both", 1), "to-out\nto-err\n");
        assert!(
            !o.stdout.lines().any(|l| l == "to-out") && !o.stderr.contains("to-err"),
            "{}",
            o.all()
        );
    }

    #[test]
    fn a_missing_command_is_a_red_run_127() {
        let g = Gate::new();
        let o = g.run("nf", &["--times", "1"], &["/nonexistent/prog"]);
        assert_eq!(o.code, 1, "{}", o.all());
        assert!(o.stdout.contains("the command exited 127"), "{}", o.stdout);
        assert!(
            g.log("nf", 1).contains("command not found"),
            "{}",
            g.log("nf", 1)
        );
    }

    #[test]
    fn without_a_log_dir_the_logs_go_to_a_new_temp_dir_that_the_report_names() {
        let g = Gate::new();
        let repo = g.repo.to_string_lossy().to_string();
        let o = out_of(
            g.cmd(&["--times", "1", "--work-dir", &repo, "--", "true"])
                .output()
                .unwrap(),
        );
        assert_eq!(o.code, 0, "{}", o.all());
        let dir = o
            .stdout
            .lines()
            .find_map(|l| l.strip_prefix("gate-loop: logs in "))
            .unwrap()
            .to_string();
        assert!(Path::new(&dir).join("run-1.log").is_file(), "{dir}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_repo_that_is_not_a_checkout_with_a_commit_exits_4() {
        let g = Gate::new();
        // a directory that is no checkout
        let plain = g.rig.p("plain");
        std::fs::create_dir(&plain).unwrap();
        let o = out_of(
            g.cmd(&[
                "--times",
                "1",
                "--work-dir",
                plain.to_str().unwrap(),
                "--",
                "true",
            ])
            .output()
            .unwrap(),
        );
        assert_eq!(o.code, 4, "{}", o.all());
        assert!(
            o.stderr.contains("is not a git checkout with a commit"),
            "{}",
            o.stderr
        );
        // a checkout without a commit
        let empty = g.rig.p("empty");
        std::fs::create_dir(&empty).unwrap();
        git(&empty, &["init", "-q"]);
        let o = out_of(
            g.cmd(&[
                "--times",
                "1",
                "--work-dir",
                empty.to_str().unwrap(),
                "--",
                "true",
            ])
            .output()
            .unwrap(),
        );
        assert_eq!(o.code, 4, "{}", o.all());
        // a path that is no directory
        let o = out_of(
            g.cmd(&[
                "--times",
                "1",
                "--work-dir",
                "/nonexistent/repo",
                "--",
                "true",
            ])
            .output()
            .unwrap(),
        );
        assert_eq!(o.code, 4, "{}", o.all());
        assert!(
            o.stderr.contains("--work-dir is not a directory"),
            "{}",
            o.stderr
        );
        // the other direction: the same command in the checkout runs
        let o = g.run("ok", &["--times", "1"], &["true"]);
        assert_eq!(o.code, 0, "{}", o.all());
    }

    #[test]
    fn bad_runs_and_missing_cmd_exit_64() {
        // G8
        let g = Gate::new();
        let repo = g.repo.to_string_lossy().to_string();
        let code = |args: &[&str]| {
            let mut a = vec!["--work-dir", repo.as_str()];
            a.extend_from_slice(args);
            out_of(g.cmd(&a).output().unwrap())
        };
        let o = code(&["--times", "0", "--", "x"]);
        assert_eq!(o.code, 64, "{}", o.all());
        assert!(
            o.stderr
                .contains("gate-loop: --times needs a whole number >= 1, got '0'")
        );
        for bad in ["abc", "-1", "1.5", ""] {
            assert_eq!(code(&["--times", bad, "--", "x"]).code, 64, "{bad}");
        }
        // no --times at all, and no command
        assert_eq!(code(&["--", "x"]).code, 64);
        let o = code(&["--times", "2"]);
        assert_eq!(o.code, 64, "{}", o.all());
        assert!(
            o.stderr.contains("a command is required after --"),
            "{}",
            o.stderr
        );
        // a bad duration, an unknown option (a usage error of the parser is 64 for the gate)
        assert_eq!(
            code(&["--times", "1", "--ci-poll", "soon", "--", "x"]).code,
            64
        );
        assert_eq!(
            code(&["--times", "1", "--lock-wait", "-5", "--", "x"]).code,
            64
        );
        assert_eq!(
            code(&["--times", "1", "--no-such-option", "--", "x"]).code,
            64
        );
        // the other direction: a good command line runs
        assert_eq!(code(&["--times", "1", "--", "true"]).code, 0);
    }

    #[test]
    fn a_bad_regex_exits_64_with_the_compile_error() {
        let g = Gate::new();
        let o = g.run(
            "re1",
            &["--times", "1", "--result-re", "(unclosed"],
            &["true"],
        );
        assert_eq!(o.code, 64, "{}", o.all());
        assert!(
            o.stderr
                .contains("--result-re '(unclosed' is not a valid pattern: regex parse error"),
            "{}",
            o.stderr
        );
        let o = g.run(
            "re2",
            &["--times", "1", "--result-re", "^ok", "--fail-re", "[z-a]"],
            &["true"],
        );
        assert_eq!(o.code, 64, "{}", o.all());
        assert!(
            o.stderr
                .contains("--fail-re '[z-a]' is not a valid pattern"),
            "{}",
            o.stderr
        );
        // nothing ran
        assert!(!g.rig.p("re1").exists());
    }

    #[test]
    fn lock_held_during_each_run() {
        // the owner file names the gate owner while the command runs, the child has the grant
        // variables, and the lock is gone after the last run
        let g = Gate::new();
        let lk = g.rig.s("g7.lock.d");
        let during = g.rig.s("g7.during");
        let script = format!(
            "grep '^agent=' {lk}/owner >>{during}; grep '^reason=' {lk}/owner >>{during}; \
             echo \"grant=$CREWBOARD_HOST_LOCK\" >>{during}; {GREEN}"
        );
        let o = g.sh(
            "g7",
            &[
                "--times",
                "2",
                "--lock",
                &lk,
                "--holder",
                "gl@x",
                "--result-re",
                CARGO_RE,
            ],
            &script,
        );
        assert_eq!(o.code, 0, "{}", o.all());
        let seen = std::fs::read_to_string(&during).unwrap();
        assert_eq!(lines_of(&seen, "agent=gl@x"), 2, "{seen}");
        assert!(seen.contains("reason=gate-loop run 1 of 2"), "{seen}");
        assert!(seen.contains("reason=gate-loop run 2 of 2"), "{seen}");
        assert_eq!(lines_of(&seen, "grant="), 2);
        assert!(!seen.contains("grant=\n"), "{seen}");
        assert!(
            !g.rig.p("g7.lock.d").exists(),
            "the lock must be free after the runs"
        );
        // the result lines of the run under the lock are in its own log
        assert_eq!(
            lines_of(&o.stdout, "  test result: ok. 5 passed"),
            2,
            "{}",
            o.stdout
        );
        // the owner defaults to $CREWBOARD_AGENT, else gate-loop
        let seen_owner = g.rig.s("g7.owner");
        let script = format!("grep '^agent=' {lk}/owner >{seen_owner}");
        let o = g.sh("g7c", &["--times", "1", "--lock", &lk], &script);
        assert_eq!(o.code, 0, "{}", o.all());
        assert_eq!(
            std::fs::read_to_string(&seen_owner).unwrap().trim(),
            "agent=gate-loop"
        );
    }

    #[test]
    fn live_owner_blocks_the_run() {
        // a live foreign owner holds the lock: the command does not start, the lock stays
        let g = Gate::new();
        let lk = g.rig.s("g7b.lock.d");
        g.rig.plant(
            "g7b.lock.d",
            "other@x",
            lock::HOST,
            g.rig.live(),
            lock::now(),
        );
        let ran = g.rig.s("g7b.ran");
        let o = g.sh(
            "g7b",
            &[
                "--times",
                "1",
                "--lock",
                &lk,
                "--holder",
                "gl@x",
                "--lock-wait",
                "0",
            ],
            &format!("touch {ran}"),
        );
        assert_eq!(o.code, 1, "{}", o.all());
        assert!(!g.rig.p("g7b.ran").exists(), "the command must not start");
        assert!(
            o.stdout
                .contains("RED at run 1 of 1: the command exited 75"),
            "{}",
            o.stdout
        );
        assert!(o.stderr.contains("held by other@x"), "{}", o.stderr);
        assert_eq!(
            lock::owner_field(&g.rig.p("g7b.lock.d"), "agent").as_deref(),
            Some("other@x")
        );
        // the log of a run that did not start exists and is empty
        assert_eq!(g.log("g7b", 1), "");
        // the other direction: after the owner leaves, the same command runs
        std::fs::remove_dir_all(&lk).unwrap();
        let o = g.sh(
            "g7b",
            &[
                "--times",
                "1",
                "--lock",
                &lk,
                "--holder",
                "gl@x",
                "--lock-wait",
                "0",
            ],
            &format!("touch {ran}"),
        );
        assert_eq!(o.code, 0, "{}", o.all());
        assert!(g.rig.p("g7b.ran").exists());
    }

    #[test]
    fn a_ci_job_pauses_the_gate_and_the_evidence_says_so() {
        // Runner.Worker runs; the gate pauses before run 1, goes on when it is gone, and does
        // not pause again before run 2
        let g = Gate::new();
        g.rig.set_runner(true);
        let gate = start(&g, "g9", &["--times", "2", "--ci-poll", "0.2"], "true");
        gate.wait_stderr("gate-loop: run 1 paused");
        g.rig.set_runner(false);
        let o = finish(gate, "end after the CI job is gone");
        assert_eq!(o.code, 0, "{}", o.all());
        assert!(
            o.stderr
                .contains("gate-loop: run 1 paused: a CI job runs on this host (Runner.Worker)"),
            "{}",
            o.stderr
        );
        assert!(
            o.stderr.contains("gate-loop: run 1 continues: CI is gone"),
            "{}",
            o.stderr
        );
        assert_eq!(
            lines_of(&o.stderr, "gate-loop: run 1 paused"),
            1,
            "{}",
            o.stderr
        );
        assert!(
            o.stdout
                .lines()
                .any(|l| l.starts_with("run 1: paused ") && l.ends_with(" s for CI, then went on")),
            "{}",
            o.stdout
        );
        assert!(!o.stdout.contains("run 2: paused"), "{}", o.stdout);
    }

    #[test]
    fn a_ci_job_that_never_ends_gives_exit_3_and_the_command_never_runs() {
        // G9b
        let g = Gate::new();
        g.rig.set_runner(true);
        let ran = g.rig.s("g9b.ran");
        let o = g.sh(
            "g9b",
            &["--times", "1", "--ci-poll", "0.2", "--ci-max-wait", "1"],
            &format!("touch {ran}"),
        );
        assert_eq!(o.code, 3, "{}", o.all());
        assert!(!g.rig.p("g9b.ran").exists());
        assert!(
            o.stdout
                .contains("gate-loop: CI did not finish within 1 s before run 1 of 1"),
            "{}",
            o.stdout
        );
        assert!(
            o.stdout
                .lines()
                .any(|l| l.starts_with("run 1: not run, paused ")
                    && l.contains(" s for CI: a CI job runs on this host")),
            "{}",
            o.stdout
        );
        assert!(!g.rig.p("g9b/run-1.log").exists());
        // --no-ci-check turns the check off: the same command runs next to the CI job
        let o = g.sh(
            "g9b2",
            &["--times", "1", "--no-ci-check"],
            &format!("touch {ran}"),
        );
        assert_eq!(o.code, 0, "{}", o.all());
        assert!(g.rig.p("g9b.ran").exists());
    }

    #[test]
    fn a_waiting_list_entry_next_to_the_lock_pauses_the_gate() {
        // `ci-wait` puts an entry in <lock>.waiting; `ci-done` lets the gate go on
        let g = Gate::new();
        let lk = g.rig.s("g9c.lock.d");
        let w = g.rig.run(&["ci-wait", &lk, "--holder", "ci-x"]);
        assert_eq!(w.code, 0, "{}", w.all());
        let gate = start(
            &g,
            "g9c",
            &[
                "--times",
                "1",
                "--lock",
                &lk,
                "--holder",
                "gl@x",
                "--ci-poll",
                "0.2",
            ],
            "true",
        );
        gate.wait_stderr("gate-loop: run 1 paused");
        let d = g.rig.run(&["ci-done", &lk, "--holder", "ci-x"]);
        assert_eq!(d.code, 0, "{}", d.all());
        let o = finish(gate, "end after ci-done");
        assert_eq!(o.code, 0, "{}", o.all());
        assert!(o.stderr.contains("waiting list entry ci-x"), "{}", o.stderr);
        // no CI, no pause
        let o = g.sh("g9d", &["--times", "1", "--ci-poll", "0.2"], "true");
        assert_eq!(o.code, 0, "{}", o.all());
        assert!(!o.stderr.contains("paused"), "{}", o.stderr);
    }

    /// A gate that runs: its stdout is piped, its stderr goes to a file, so that a test can wait
    /// for a line of it while the gate runs.
    struct Started {
        child: std::process::Child,
        err: PathBuf,
    }

    impl Started {
        /// Wait until the gate has printed `text` on stderr (at most 20 s).
        fn wait_stderr(&self, text: &str) {
            let seen = || std::fs::read_to_string(&self.err).is_ok_and(|t| t.contains(text));
            assert!(
                lock::wait_for(20, seen),
                "the gate did not say {text:?}: {:?}",
                std::fs::read_to_string(&self.err)
            );
        }
    }

    /// Start the gate (not wait for it) with piped stdout and stderr in a file.
    fn start(g: &Gate, name: &str, args: &[&str], script: &str) -> Started {
        let logs = g.logs(name);
        let repo = g.repo.to_string_lossy().to_string();
        let mut a = vec!["--work-dir", &repo, "--log-dir", &logs];
        a.extend_from_slice(args);
        a.extend(["--", "sh", "-c", script]);
        let err = g.rig.p(&format!("{name}.stderr"));
        let child = g
            .cmd(&a)
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(&err).unwrap())
            .spawn()
            .unwrap();
        Started { child, err }
    }

    /// Wait for the gate to end, at most 20 s. A gate that does not end is killed and the test
    /// fails with what it was waiting for.
    fn finish(started: Started, expected: &str) -> Out {
        use std::io::Read;
        let Started { mut child, err } = started;
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let status = loop {
            if let Some(st) = child.try_wait().unwrap() {
                break st;
            }
            if std::time::Instant::now() >= end {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the gate did not end within 20 s: expected it to {expected}");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        let read = |r: Option<&mut dyn Read>| {
            let mut t = String::new();
            if let Some(r) = r {
                let _ = r.read_to_string(&mut t);
            }
            t
        };
        let stdout = read(child.stdout.as_mut().map(|s| s as &mut dyn Read));
        let stderr = std::fs::read_to_string(err).unwrap_or_default();
        Out {
            code: status.code().unwrap_or(-1),
            stdout,
            stderr,
        }
    }

    fn term(started: &Started) {
        let child = &started.child;
        // SAFETY: kill has no memory effect.
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
    }

    /// The pid that a `sleep 30` command wrote, once it is there.
    fn child_pid(file: &Path) -> u32 {
        assert!(
            lock::wait_for(10, || std::fs::read_to_string(file)
                .is_ok_and(|t| !t.trim().is_empty())),
            "the command did not start"
        );
        std::fs::read_to_string(file)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    #[test]
    fn term_stops_the_gate_kills_the_command_and_prints_the_evidence() {
        let g = Gate::new();
        let pidfile = g.rig.s("t1.pid");
        let gate = start(
            &g,
            "t1",
            &["--times", "3"],
            &format!("echo $$ >{pidfile}; exec sleep 30"),
        );
        let pid = child_pid(Path::new(&pidfile));
        term(&gate);
        let o = finish(gate, "stop on the signal");
        assert_eq!(o.code, 143, "{}", o.all());
        assert!(
            lock::wait_for(5, || !lock::pid_alive(pid)),
            "the command must be dead"
        );
        assert!(
            o.stdout.contains(
                "gate-loop: STOPPED by signal 15 during run 1 of 3: run 1 is not counted"
            ),
            "{}",
            o.stdout
        );
        assert!(
            o.stdout
                .lines()
                .any(|l| l.starts_with("run 1: sha=") && l.contains(" exit=143 ")),
            "{}",
            o.stdout
        );
        assert!(!g.rig.p("t1/run-2.log").exists(), "no run 2");
    }

    #[test]
    fn term_stops_a_locked_gate_kills_the_command_and_frees_the_lock() {
        let g = Gate::new();
        let lk = g.rig.s("t2.lock.d");
        let pidfile = g.rig.s("t2.pid");
        let gate = start(
            &g,
            "t2",
            &["--times", "3", "--lock", &lk, "--holder", "gl@x"],
            &format!("echo $$ >{pidfile}; exec sleep 30"),
        );
        let pid = child_pid(Path::new(&pidfile));
        assert!(
            g.rig.p("t2.lock.d/owner").is_file(),
            "the lock is held while the command runs"
        );
        term(&gate);
        let o = finish(gate, "stop on the signal");
        assert_eq!(o.code, 143, "{}", o.all());
        assert!(
            lock::wait_for(5, || !lock::pid_alive(pid)),
            "the command must be dead"
        );
        assert!(!g.rig.p("t2.lock.d").exists(), "the lock must be free");
        assert!(
            o.stdout
                .contains("gate-loop: STOPPED by signal 15 during run 1 of 3"),
            "{}",
            o.stdout
        );
        assert!(
            o.stdout.lines().any(|l| l.starts_with("run 1: sha=")),
            "{}",
            o.stdout
        );
    }

    #[test]
    fn term_ends_the_wait_for_a_live_lock_owner() {
        let g = Gate::new();
        let lk = g.rig.s("t3.lock.d");
        g.rig.plant(
            "t3.lock.d",
            "other@x",
            lock::HOST,
            g.rig.live(),
            lock::now(),
        );
        let ran = g.rig.s("t3.ran");
        let gate = start(
            &g,
            "t3",
            &[
                "--times",
                "1",
                "--lock",
                &lk,
                "--holder",
                "gl@x",
                "--lock-wait",
                "60",
            ],
            &format!("touch {ran}"),
        );
        gate.wait_stderr("waiting for lock");
        term(&gate);
        let o = finish(gate, "stop on the signal");
        assert_eq!(o.code, 143, "{}", o.all());
        assert!(!g.rig.p("t3.ran").exists(), "the command must not start");
        assert_eq!(
            lock::owner_field(&g.rig.p("t3.lock.d"), "agent").as_deref(),
            Some("other@x")
        );
        assert!(
            o.stdout.contains("gate-loop: STOPPED by signal 15"),
            "{}",
            o.stdout
        );
    }

    #[test]
    fn term_ends_the_pause_for_ci() {
        let g = Gate::new();
        g.rig.set_runner(true);
        let ran = g.rig.s("t4.ran");
        let gate = start(
            &g,
            "t4",
            &["--times", "1", "--ci-poll", "30"],
            &format!("touch {ran}"),
        );
        gate.wait_stderr("gate-loop: run 1 paused");
        term(&gate);
        let o = finish(gate, "stop on the signal");
        assert_eq!(o.code, 143, "{}", o.all());
        assert!(!g.rig.p("t4.ran").exists());
        assert!(
            o.stdout
                .contains("gate-loop: STOPPED by signal 15 before run 1 of 1"),
            "{}",
            o.stdout
        );
    }

    #[test]
    fn a_locked_command_that_cannot_start_is_127_with_the_error_in_the_log() {
        let g = Gate::new();
        let lk = g.rig.s("nf2.lock.d");
        let o = g.run(
            "nf2",
            &["--times", "1", "--lock", &lk, "--holder", "gl@x"],
            &["/nonexistent/prog"],
        );
        assert_eq!(o.code, 1, "{}", o.all());
        assert!(o.stdout.contains("the command exited 127"), "{}", o.stdout);
        assert_eq!(g.log("nf2", 1), "/nonexistent/prog: command not found\n");
        assert!(!g.rig.p("nf2.lock.d").exists(), "the lock must be free");
    }

    #[test]
    fn a_ci_poll_of_zero_does_not_spin() {
        // the minimum is 100 ms: the gate pauses, and ends at --ci-max-wait without a busy loop
        let g = Gate::new();
        g.rig.set_runner(true);
        let t = std::time::Instant::now();
        let o = g.sh(
            "poll0",
            &["--times", "1", "--ci-poll", "0", "--ci-max-wait", "1"],
            "true",
        );
        assert_eq!(o.code, 3, "{}", o.all());
        assert!(t.elapsed() >= std::time::Duration::from_millis(900));
    }
}
