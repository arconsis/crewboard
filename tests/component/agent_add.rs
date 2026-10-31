//! `crewboard agent add`: the seven steps against the fake GitHub, each with its red proof, and
//! the headless-coordinator form.
//!
//! Each test has its own temp dir with an env file, a fake `gh` first on the PATH, and a local
//! repo that stands for `https://github.com/<repo>` (git's own `url.<x>.insteadOf` through
//! `GIT_CONFIG_COUNT`). Nothing reaches github.com. `start` runs as a child of the binary under
//! test, against the same fake. Every child of a test has a deadline: a hang fails the test.
#![cfg(unix)]

use crate::support::board::{FakeBoard, INITIATIVE, Out, REPO};
use crewboard::rules::Role;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const FILE: &str = ".crewboard.toml";
const NAME: &str = "claude@fake-a";
const TOKEN: &str = "add-token-SECRET-123456";
/// `GET users/{login}` of the fake answers id 2; `GET user` answers the owner.
const EMAIL: &str = "2+fake-owner@users.noreply.github.com";

pub(crate) fn write_exe(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Run a command to its end, or kill it after `limit` and fail the test.
pub(crate) fn run_deadline(c: Command, limit: Duration) -> Out {
    run_deadline_stdin(c, limit, None)
}

/// [`run_deadline`] with `input` on standard input (default: none).
pub(crate) fn run_deadline_stdin(mut c: Command, limit: Duration, input: Option<&str>) -> Out {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let (so, se) = (dir.path().join("out"), dir.path().join("err"));
    c.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(std::fs::File::create(&so).unwrap())
    .stderr(std::fs::File::create(&se).unwrap());
    let mut child = c.spawn().expect("run crewboard");
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(text.as_bytes()).unwrap();
    }
    let start = Instant::now();
    let code = loop {
        match child.try_wait().unwrap() {
            Some(s) => break s.code().unwrap_or(-1),
            None if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "the command ran longer than {limit:?}\nstdout:\n{}\nstderr:\n{}",
                    std::fs::read_to_string(&so).unwrap_or_default(),
                    std::fs::read_to_string(&se).unwrap_or_default()
                );
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    Out {
        code,
        stdout: std::fs::read_to_string(&so).unwrap_or_default(),
        stderr: std::fs::read_to_string(&se).unwrap_or_default(),
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(o.status.success(), "git {args:?}: {o:?}");
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

pub(crate) struct Setup {
    pub fake: FakeBoard,
    pub tmp: tempfile::TempDir,
    pub env_file: PathBuf,
    pub realgh: PathBuf,
}

impl Setup {
    pub fn new() -> Self {
        let fake = FakeBoard::new();
        fake.set_file(REPO, FILE, FakeBoard::board_toml());
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        let env_file = t.join("crew.env");
        std::fs::write(&env_file, format!("GH_TOKEN={TOKEN}\n")).unwrap();
        std::fs::set_permissions(&env_file, std::fs::Permissions::from_mode(0o600)).unwrap();
        // the fake real gh: says which token it saw, answers the two lookups of the seam, and
        // prints the owner's own token for `gh auth token` (the owner commands)
        let realgh = t.join("realgh");
        write_exe(
            &realgh.join("gh"),
            "#!/bin/sh\ncase \"$*\" in\n  \"api user --jq .login\") echo crew-bot ;;\n  \"api users/crew-bot --jq .id\") echo 4242 ;;\n  \"api users/nobody --jq .id\") echo null ;;\n  \"auth token\") echo ghp_owner_gh_login ;;\n  *) echo \"gh-saw token=${GH_TOKEN:-unset} args=$*\" ;;\nesac\n",
        );
        // the repo that `https://github.com/<repo>` stands for
        let remote = t.join("remote").join(REPO);
        std::fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "-q"]);
        git(
            &remote,
            &[
                "-c",
                "user.name=o",
                "-c",
                "user.email=o@x",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        std::fs::create_dir_all(t.join("home")).unwrap();
        Setup {
            fake,
            tmp,
            env_file,
            realgh,
        }
    }

    pub fn path(&self, p: &str) -> PathBuf {
        self.tmp.path().join(p)
    }

    /// The common arguments of a dev run: repo, agent dir, work dir, env file, name, runtime.
    pub fn args(&self, agent: &str, work: &str) -> Vec<String> {
        [
            "agent".to_string(),
            "add".into(),
            REPO.into(),
            "--agent-dir".into(),
            self.path(agent).display().to_string(),
            "--work-dir".into(),
            self.path(work).display().to_string(),
            "--env-file".into(),
            self.env_file.display().to_string(),
            "--agent".into(),
            NAME.into(),
            "--runtime".into(),
            "claude".into(),
        ]
        .to_vec()
    }

    pub fn run(&self, args: &[String], env: &[(&str, &str)]) -> Out {
        self.run_stdin(args, env, None)
    }

    pub fn run_stdin(&self, args: &[String], env: &[(&str, &str)], input: Option<&str>) -> Out {
        let t = self.tmp.path();
        let mut c = Command::new(env!("CARGO_BIN_EXE_crewboard"));
        c.args(args)
            .current_dir(t)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", self.realgh.display()))
            .env("HOME", t.join("home"))
            .env("CREWBOARD_API_BASE", self.fake.base())
            // an owner token of the caller must never reach the agent
            .env("GH_TOKEN", "ghp_caller_owner_token")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_COUNT", "1")
            .env(
                "GIT_CONFIG_KEY_0",
                format!("url.{}/.insteadOf", t.join("remote").display()),
            )
            .env("GIT_CONFIG_VALUE_0", "https://github.com/")
            .envs(env.iter().copied());
        run_deadline_stdin(c, Duration::from_secs(120), input)
    }

    pub fn add(&self, agent: &str, work: &str, extra: &[&str]) -> Out {
        let mut a = self.args(agent, work);
        a.extend(extra.iter().map(|s| s.to_string()));
        self.run(&a, &[])
    }

    /// The owner approves the agent (the owner's own config dir).
    pub fn approve(&self, name: &str, extra: &[&str]) -> Out {
        let owner = self.fake.owner_dir();
        let mut a = vec![
            "--config-dir".to_string(),
            owner.path().display().to_string(),
            "approve".into(),
            name.into(),
            "--initiative".into(),
            INITIATIVE.into(),
        ];
        a.extend(extra.iter().map(|s| s.to_string()));
        let o = self.run(&a, &[]);
        assert_eq!(o.code, 0, "approve: {}", o.all());
        o
    }
}

pub(crate) fn steps(o: &Out, word: &str) -> usize {
    o.stdout
        .lines()
        .filter(|l| {
            l.starts_with("agent add: [")
                && l.contains("/7] ")
                && l.split(": ").nth(2).is_some_and(|s| s.starts_with(word))
        })
        .count()
}

fn next_steps(o: &Out) -> usize {
    o.stdout
        .lines()
        .filter(|l| l.starts_with("agent add: next step:"))
        .count()
}

fn all_text(dir: &Path) -> String {
    let mut s = String::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        // `agent.env` is a link to the env file: it holds the token by design
        if p.symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            continue;
        }
        if p.is_dir() {
            s.push_str(&all_text(&p));
        } else if let Ok(t) = std::fs::read_to_string(&p) {
            s.push_str(&t);
        }
    }
    s
}

#[test]
fn b1_seven_steps_join_posted_one_next_step() {
    let s = Setup::new();
    let o = s.add("agents/dev1", "work", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stderr.is_empty(), "{}", o.all());
    assert_eq!(steps(&o, "done"), 6, "{}", o.stdout);
    assert!(
        o.stdout
            .contains("[7/7] start: not yet: the owner has not approved claude@fake-a"),
        "{}",
        o.stdout
    );
    assert_eq!(next_steps(&o), 1, "{}", o.stdout);
    let last = o.stdout.lines().last().unwrap();
    assert!(
        last.starts_with(&format!(
            "agent add: next step: ask the owner to approve {NAME} (owner: crewboard approve {NAME} --initiative fake;"
        )),
        "{last}"
    );
    assert!(
        last.contains("then run: crewboard agent add fake-owner/fake-repo "),
        "{last}"
    );
    // the effects
    let dir = s.path("agents/dev1");
    assert_eq!(
        std::fs::read_link(dir.join("agent.env")).unwrap(),
        s.env_file
    );
    assert_eq!(mode(&dir), 0o700);
    assert!(
        dir.join("fake.toml").is_file(),
        "the join wrote the agent config"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("crew-id")).unwrap(),
        "fake-owner 2\n"
    );
    let stub = std::fs::read_to_string(dir.join("bin/gh")).unwrap();
    assert_eq!(
        stub,
        format!(
            "#!/bin/sh\n# crewboard gh shim for {d}: runs the real gh as the agent account.\nexec '{exe}' agent with-env --env-file '{d}/agent.env' -- '{gh}' \"$@\"\n",
            d = dir.display(),
            exe = env!("CARGO_BIN_EXE_crewboard"),
            gh = s.realgh.join("gh").display()
        )
    );
    assert_eq!(mode(&dir.join("bin/gh")), 0o755);
    assert_eq!(mode(&dir.join("gh-empty-config")), 0o700);
    let wd = s.path("work");
    assert_eq!(git(&wd, &["config", "user.name"]), "fake-owner");
    assert_eq!(git(&wd, &["config", "user.email"]), EMAIL);
    assert_eq!(
        git(&wd, &["config", "--get-all", "credential.helper"])
            .lines()
            .last()
            .unwrap(),
        format!("!{}/bin/gh auth git-credential", dir.display())
    );
    assert_eq!(
        git(
            &wd,
            &["config", "--get", "url.https://github.com/.insteadOf"]
        ),
        "git@github.com:"
    );
    // the join request is on the board, with the label of the runtime claude without a model
    assert!(
        s.fake
            .requests()
            .iter()
            .any(|r| r.method == "POST" && r.path.contains("/comments")),
        "no comment was posted"
    );
    assert!(
        s.fake
            .comments_of(crate::support::board::REGISTRY)
            .iter()
            .any(|(_, _, b)| b.contains("Join request.") && b.contains("\nruntime=claude-code\n")),
        "no join request with runtime=claude-code"
    );
    assert!(s.fake.unhandled().is_empty(), "{:?}", s.fake.unhandled());
    // the log: mode 600, one block for each step that ran a command
    let log = dir.join("bootstrap.log");
    assert_eq!(mode(&log), 0o600);
    assert!(
        std::fs::read_to_string(&log).unwrap().contains("== "),
        "no header in the log"
    );
}

#[test]
fn b1b_no_token_in_output_or_log() {
    let s = Setup::new();
    let o = s.add("agents/dev1", "work", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!o.all().contains(TOKEN), "{}", o.all());
    assert!(
        !all_text(&s.path("agents/dev1")).contains(TOKEN),
        "a file of the agent dir holds the token"
    );
    assert!(
        !all_text(&s.path("work/.git")).contains(TOKEN),
        "the work dir holds the token"
    );
    // the owner token of the caller never reaches the agent either
    assert!(!o.all().contains("ghp_caller_owner_token"), "{}", o.all());
}

/// A seam binary that `join` and `start` go through: one script, behaviour by `$FAKE`.
fn seam(s: &Setup, body: &str) -> String {
    let p = s.path("seam/crewboard");
    write_exe(&p, &format!("#!/bin/sh\n{body}\n"));
    p.display().to_string()
}

#[test]
fn b1c_a_token_that_a_step_prints_is_replaced_in_the_log_and_in_the_failure_text() {
    let s = Setup::new();
    // join fails and prints the token it was given, as a careless tool could
    let bin = seam(&s, "echo \"join saw $GH_TOKEN\" >&2\nexit 3");
    let o = s.add("agents/dev1", "work", &["--crewboard", &bin]);
    assert_eq!(o.code, 3, "{}", o.all());
    assert!(
        o.stderr.contains("[2/7] join: FAIL: crewboard join exit 3"),
        "{}",
        o.all()
    );
    assert!(
        o.stderr.contains("join saw ***"),
        "the failure text shows the line with the token replaced: {}",
        o.all()
    );
    assert!(!o.all().contains(TOKEN), "{}", o.all());
    let log = std::fs::read_to_string(s.path("agents/dev1/bootstrap.log")).unwrap();
    assert!(log.contains("join saw ***"), "{log}");
    assert!(!log.contains(TOKEN), "{log}");
}

#[test]
fn b2_rerun_all_already_done_and_no_second_join() {
    let s = Setup::new();
    assert_eq!(s.add("agents/dev1", "work", &[]).code, 0);
    let writes = crate::support::cli::writes(&s.fake);
    let o = s.add("agents/dev1", "work", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(steps(&o, "already done"), 6, "{}", o.stdout);
    assert_eq!(
        steps(&o, "done"),
        0,
        "no step may say plain `done`: {}",
        o.stdout
    );
    assert!(o.stdout.contains("[7/7] start: not yet"), "{}", o.stdout);
    assert_eq!(next_steps(&o), 1, "{}", o.stdout);
    assert_eq!(
        crate::support::cli::writes(&s.fake),
        writes,
        "the rerun wrote to the board"
    );
}

#[test]
fn b3_rerun_after_approval_start_is_done_and_the_next_step_runs_the_runtime() {
    let s = Setup::new();
    assert_eq!(s.add("agents/dev1", "work", &[]).code, 0);
    s.approve(NAME, &[]);
    let o = s.add("agents/dev1", "work", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .contains(&format!("[7/7] start: done ({NAME} is active)")),
        "{}",
        o.all()
    );
    assert_eq!(next_steps(&o), 1, "{}", o.stdout);
    let lines: Vec<&str> = o.stdout.lines().collect();
    let n = lines.len();
    let dir = s.path("agents/dev1");
    // the introduction, the bare command on its own line, then the optional hint
    assert_eq!(
        lines[n - 3],
        "agent add: next step: start the runtime in the work dir:"
    );
    assert_eq!(
        lines[n - 2],
        format!(
            "cd '{}' && crewboard agent exec --agent-dir '{}' -- claude 'Use the crewboard skill. You are {NAME}, a dev agent of fake. Run `crewboard resume`. Then start the watch command that it prints under Monitor (persistent).'",
            s.path("work").display(),
            dir.display()
        )
    );
    assert!(
        lines[n - 1].starts_with(
            "agent add: optional: an unattended agent: crewboard wake install --scheduler "
        ),
        "{}",
        lines[n - 1]
    );
}

#[test]
fn approve_of_an_active_agent_says_nothing_to_do_and_writes_nothing() {
    let s = Setup::new();
    assert_eq!(s.add("agents/dev1", "work", &[]).code, 0);
    // a first approval keeps today's output
    let first = s.approve(NAME, &[]);
    assert!(first.stdout.contains("approved as dev"), "{}", first.all());
    assert!(
        first.stdout.contains("next step: tell the agent host"),
        "{}",
        first.all()
    );
    // not active yet: a second approve still has its next step
    let again = s.approve(NAME, &[]);
    assert!(again.stdout.contains("approved as dev"), "{}", again.all());
    assert!(
        again.stdout.contains("next step: tell the agent host"),
        "{}",
        again.all()
    );
    // the agent starts: it is active
    let o = s.add("agents/dev1", "work", &[]);
    assert!(
        o.stdout
            .contains(&format!("[7/7] start: done ({NAME} is active)")),
        "{}",
        o.all()
    );
    let writes = crate::support::cli::writes(&s.fake);
    let o = s.approve(NAME, &[]);
    assert_eq!(
        o.stdout.trim(),
        format!("{NAME} is already approved and active; nothing to do"),
        "{}",
        o.all()
    );
    assert!(!o.stdout.contains("next step"), "{}", o.all());
    assert_eq!(
        crate::support::cli::writes(&s.fake),
        writes,
        "the second approve wrote to the board"
    );
}

#[test]
fn b4_start_refused_names_the_step_and_doctor_as_the_fix_and_keeps_its_exit_code() {
    let s = Setup::new();
    assert_eq!(s.add("agents/dev1", "work", &[]).code, 0);
    // join is done already; only `start` goes through the seam, and it refuses for another reason
    let bin = seam(
        &s,
        "echo 'refused [rule=agent-gh-not-crew]: plain gh acts as the owner' >&2\nexit 2",
    );
    let o = s.add("agents/dev1", "work", &["--crewboard", &bin]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr
            .contains("[7/7] start: FAIL: crewboard start exit 2"),
        "{}",
        o.all()
    );
    assert!(o.stderr.contains("agent-gh-not-crew"), "{}", o.all());
    assert!(o.stderr.contains("doctor --setup"), "{}", o.all());
    assert!(!o.stdout.contains("next step"), "{}", o.all());
    // twin: the refusal that waits for the owner is no failure (b2)
    let bin = seam(
        &s,
        "echo 'refused [rule=sender-not-approved]: x is not an approved agent: the owner has not approved it yet' >&2\nexit 2",
    );
    let o = s.add("agents/dev1", "work", &["--crewboard", &bin]);
    assert_eq!(o.code, 0, "{}", o.all());
    // twin: the approval is posted but `approve` has not written the State yet: still waiting
    let bin = seam(
        &s,
        "echo \"refused [rule=agent-approval-pending]: x: the owner's approval is in the registry log, but the item State is still 'requested' (\\`approve\\` writes 'approved' a moment later); run \\`crewboard start\\` again\" >&2\nexit 2",
    );
    let o = s.add("agents/dev1", "work", &["--crewboard", &bin]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!o.stderr.contains("FAIL"), "{}", o.all());
    // twin: any other State that may not speak is a failure at once
    let bin = seam(
        &s,
        "echo \"refused [rule=sender-not-approved]: x is not an approved agent: approved, but not trusted (x: item State is 'left' but the registry log has not ended this agent)\" >&2\nexit 2",
    );
    let o = s.add("agents/dev1", "work", &["--crewboard", &bin]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("[7/7] start: FAIL"), "{}", o.all());
}

#[test]
fn b5_env_file_mode_644_fails_step_1_and_nothing_runs() {
    let s = Setup::new();
    std::fs::set_permissions(&s.env_file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let o = s.add("agents/dev1", "work", &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("[1/7] agent dir: FAIL"), "{}", o.all());
    assert!(
        o.stderr
            .contains(&format!("fix: chmod 600 {}", s.env_file.display())),
        "{}",
        o.all()
    );
    assert!(!s.path("agents/dev1").exists());
    assert!(s.fake.requests().is_empty(), "a request went out");
    // twin: mode 600 passes
    std::fs::set_permissions(&s.env_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(s.add("agents/dev1", "work", &[]).code, 0);
}

#[test]
fn b5a_a_missing_env_file_fails_step_1_with_the_way_to_write_it() {
    let s = Setup::new();
    std::fs::remove_file(&s.env_file).unwrap();
    let o = s.add("agents/dev1", "work", &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr
            .contains("[1/7] agent dir: FAIL: env file not found"),
        "{}",
        o.all()
    );
    assert!(
        o.stderr
            .contains("crewboard agent token --initiative <initiative>")
            && o.stderr.contains("--token-stdin"),
        "{}",
        o.all()
    );
}

#[test]
fn b5b_join_fails_step_2_with_the_code_of_join_and_no_shim_no_clone() {
    let s = Setup::new();
    s.fake.fail_file(REPO, FILE, 500);
    let o = s.add("agents/dev1", "work", &["--initiative", INITIATIVE]);
    assert!(o.code == 3 || o.code == 4, "{}", o.all());
    assert!(
        o.stderr
            .contains(&format!("[2/7] join: FAIL: crewboard join exit {}", o.code)),
        "{}",
        o.all()
    );
    assert!(
        o.stderr.contains("fix: ") && o.stderr.contains("--board-file"),
        "{}",
        o.all()
    );
    assert!(!s.path("agents/dev1/bin/gh").exists());
    assert!(!s.path("work").exists());
    // the failure is in the log
    assert!(s.path("agents/dev1/bootstrap.log").is_file());
}

#[test]
fn b5d_agent_add_passes_runtime_and_model_to_join_in_the_new_form() {
    let s = Setup::new();
    let calls = s.path("seam/calls");
    // the seam: record each call; join writes the config
    let bin = seam(
        &s,
        &format!(
            "echo \"$*\" >>'{}'\ncase \"$*\" in *\" join \"*) printf 'name = \"claude@fake-a\"\\n' >\"$2/fake.toml\"; exit 0 ;; esac\nexit 0",
            calls.display()
        ),
    );
    let o = s.add(
        "agents/dev1",
        "work",
        &["--crewboard", &bin, "--model", "m-1"],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    let text = std::fs::read_to_string(&calls).unwrap();
    let join = text
        .lines()
        .find(|l| l.contains(" join "))
        .unwrap_or_else(|| panic!("no join call: {text}"));
    assert_eq!(
        join,
        format!(
            "--config-dir {} join --name {NAME} --runtime claude --model m-1 --repo {REPO}",
            s.path("agents/dev1").display()
        )
    );
    assert!(!text.contains("claude-code"), "{text}");
}

#[test]
fn b5c_a_crew_login_without_an_id_fails_step_4_and_the_work_dir_is_untouched() {
    let s = Setup::new();
    // the seam: join writes the config; the id comes from the fake gh through the stub
    let bin = seam(
        &s,
        "case \"$*\" in *\" join \"*) printf 'name = \"claude@fake-a\"\\n' >\"$2/fake.toml\"; exit 0 ;; esac\nexit 0",
    );
    let o = s.add(
        "agents/dev1",
        "work",
        &["--crewboard", &bin, "--crew-login", "nobody"],
    );
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr
            .contains("[4/7] crew id: FAIL: no numeric id for nobody"),
        "{}",
        o.all()
    );
    assert!(!s.path("work").exists(), "step 5 must not run");
    assert!(!s.path("agents/dev1/crew-id").exists());
    // twin: a login that has an id passes, and the id is cached
    let o = s.add(
        "agents/dev1",
        "work",
        &["--crewboard", &bin, "--crew-login", "crew-bot"],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        std::fs::read_to_string(s.path("agents/dev1/crew-id")).unwrap(),
        "crew-bot 4242\n"
    );
    assert_eq!(
        git(&s.path("work"), &["config", "user.email"]),
        "4242+crew-bot@users.noreply.github.com"
    );
}

#[test]
fn b5d_a_non_empty_work_dir_that_is_no_checkout_fails_step_5() {
    let s = Setup::new();
    std::fs::create_dir_all(s.path("notgit")).unwrap();
    std::fs::write(s.path("notgit/keep"), "x").unwrap();
    let o = s.add("agents/dev1", "notgit", &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr.contains("[5/7] work dir: FAIL:")
            && o.stderr.contains("is not empty and is not a git checkout"),
        "{}",
        o.all()
    );
    assert!(!s.path("notgit/.git").exists());
}

#[test]
fn b5d2_a_missing_parent_of_the_work_dir_is_created_and_said_in_one_line() {
    use std::os::unix::fs::PermissionsExt;
    let s = Setup::new();
    let o = s.add("agents/dev1", "crew/deep/work", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    let line = o
        .stdout
        .lines()
        .find(|l| l.contains("[5/7] work dir: done"))
        .unwrap_or_else(|| panic!("{}", o.all()));
    assert!(
        line.contains(&format!("created {};", s.path("crew").display())),
        "{line}"
    );
    for d in ["crew", "crew/deep"] {
        let mode = std::fs::metadata(s.path(d)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "{d}");
    }
    assert!(s.path("crew/deep/work/.git").exists());
}

#[test]
fn b5d3_a_missing_parent_inside_another_checkout_or_under_a_file_is_refused() {
    let s = Setup::new();
    // inside the checkout of something else: probably the wrong path
    std::fs::create_dir_all(s.path("other")).unwrap();
    git(&s.path("other"), &["init", "-q"]);
    let o = s.add("agents/dev1", "other/sub/work", &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr.contains("[5/7] work dir: FAIL:") && o.stderr.contains("inside the git checkout"),
        "{}",
        o.all()
    );
    assert!(!s.path("other/sub").exists());
    // under a plain file
    std::fs::write(s.path("afile"), "x").unwrap();
    let o = s.add("agents/dev1", "afile/work", &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("is not a directory"), "{}", o.all());
}

#[test]
fn b5e_a_checkout_of_another_repo_fails_step_5() {
    let s = Setup::new();
    std::fs::create_dir_all(s.path("other")).unwrap();
    git(&s.path("other"), &["init", "-q"]);
    git(
        &s.path("other"),
        &["remote", "add", "origin", "https://github.com/someone/else"],
    );
    let o = s.add("agents/dev1", "other", &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr.contains("[5/7] work dir: FAIL:")
            && o.stderr
                .contains("someone/else, not of fake-owner/fake-repo"),
        "{}",
        o.all()
    );
}

#[test]
fn b5f_a_changed_git_email_is_set_again_by_step_6() {
    let s = Setup::new();
    assert_eq!(s.add("agents/dev1", "work", &[]).code, 0);
    git(
        &s.path("work"),
        &["config", "user.email", "someone@example.org"],
    );
    let o = s.add("agents/dev1", "work", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("[6/7] git config: done"), "{}", o.stdout);
    assert_eq!(git(&s.path("work"), &["config", "user.email"]), EMAIL);
}

#[test]
fn b6_usage_errors_exit_4_and_write_nothing() {
    let s = Setup::new();
    let run = |args: &[&str]| {
        let a: Vec<String> = args.iter().map(|x| x.to_string()).collect();
        s.run(&a, &[])
    };
    let dir = s.path("agents/dev1").display().to_string();
    let wd = s.path("work").display().to_string();
    let env = s.env_file.display().to_string();
    // no repo and no board file
    let o = run(&[
        "agent",
        "add",
        "--agent-dir",
        &dir,
        "--work-dir",
        &wd,
        "--env-file",
        &env,
        "--runtime",
        "claude",
    ]);
    assert_eq!(
        o.code,
        64,
        "a missing argument is a usage error: {}",
        o.all()
    );
    assert!(
        o.stderr
            .contains("a repository <owner>/<repo> or a board file is required"),
        "{}",
        o.all()
    );
    // a runtime that does not exist (clap)
    let o = run(&[
        "agent",
        "add",
        REPO,
        "--agent-dir",
        &dir,
        "--runtime",
        "vim",
        "--env-file",
        &env,
    ]);
    assert_eq!(o.code, 64, "{}", o.all());
    assert!(
        o.stderr.contains("vim") && o.stderr.contains("claude"),
        "{}",
        o.all()
    );
    // a bad name
    let o = run(&[
        "agent",
        "add",
        REPO,
        "--agent-dir",
        &dir,
        "--runtime",
        "claude",
        "--agent",
        "Bad",
        "--env-file",
        &env,
        "--initiative",
        "fake",
    ]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("--agent"), "{}", o.all());
    // the default env file does not exist: it is required, and a secret path is not guessed
    let o = run(&[
        "agent",
        "add",
        REPO,
        "--agent-dir",
        &dir,
        "--runtime",
        "claude",
        "--initiative",
        "fake",
    ]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("--env-file is required"), "{}", o.all());
    // the repo form without a token, an initiative or a board file cannot read the board config
    let o = run(&[
        "agent",
        "add",
        REPO,
        "--agent-dir",
        &dir,
        "--runtime",
        "claude",
    ]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("without a token"), "{}", o.all());
    assert!(!s.path("agents").exists() && !s.path("work").exists());
    assert!(s.fake.requests().is_empty());
}

#[test]
fn defaults_come_from_the_board_config_when_only_the_repo_and_the_env_file_are_given() {
    let s = Setup::new();
    // the env file name of the default is <init>.env; the initiative is read from the repo
    let a: Vec<String> = [
        "agent",
        "add",
        REPO,
        "--env-file",
        &s.env_file.display().to_string(),
        "--runtime",
        "claude",
    ]
    .map(String::from)
    .to_vec();
    let o = s.run(&a, &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    let dir = s.path("home/.config/crewboard/agents/fake-claude");
    assert!(dir.join("fake.toml").is_file(), "{}", o.all());
    assert!(
        s.path("home/crew/fake-fake-claude/.git").is_dir(),
        "{}",
        o.all()
    );
    let name = std::fs::read_to_string(dir.join("fake.toml")).unwrap();
    let host =
        String::from_utf8_lossy(&Command::new("hostname").output().unwrap().stdout).to_string();
    let host: String = host
        .trim()
        .split('.')
        .next()
        .unwrap()
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                c
            } else {
                '-'
            }
        })
        .collect();
    assert!(
        name.contains(&format!("name = \"claude@{host}\"")),
        "{name}"
    );
}

#[test]
fn a_board_file_is_enough_and_its_main_repo_is_cloned() {
    let s = Setup::new();
    let bf = s.path("fake.board.toml");
    std::fs::write(&bf, FakeBoard::board_toml()).unwrap();
    let mut a = s.args("agents/dev1", "work");
    a[2] = bf.display().to_string();
    let o = s.run(&a, &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(steps(&o, "done"), 6, "{}", o.stdout);
    assert!(s.path("work/.git").is_dir());
    // the join read the board file, not the repo
    assert!(
        !s.fake
            .requests()
            .iter()
            .any(|r| r.path.contains(&format!("/contents/{FILE}"))),
        "the board file was read from the repo"
    );
    let last = o.stdout.lines().last().unwrap();
    assert!(
        last.contains(&format!("agent add '{}' --repo {REPO}", bf.display())),
        "{last}"
    );
}

#[test]
fn b7_coordinator_role_names_its_role_in_the_approve_line_and_the_headless_command_after() {
    let s = Setup::new();
    let o = s.add("agents/coord", "workc", &["--role", "coordinator"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(steps(&o, "done"), 6, "{}", o.stdout);
    let last = o.stdout.lines().last().unwrap();
    assert!(
        last.contains(&format!(
            "(owner: crewboard approve {NAME} --initiative fake --role coordinator)"
        )),
        "{last}"
    );
    assert!(last.ends_with("--role coordinator"), "{last}");
    s.approve(NAME, &["--role", "coordinator"]);
    let o = s.add("agents/coord", "workc", &["--role", "coordinator"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(steps(&o, "already done"), 6, "{}", o.stdout);
    assert!(o.stdout.contains("[7/7] start: done"), "{}", o.all());
    let last = o.stdout.lines().last().unwrap();
    assert!(
        last.contains(&format!(
            "crewboard agent add --role headless-coordinator --from '{}' --agent-dir '{}-headless' --env-file '{}'",
            s.path("agents/coord").display(),
            s.path("agents/coord").display(),
            s.env_file.display()
        )),
        "{last}"
    );
    assert!(
        last.starts_with("agent add: optional: the headless tick"),
        "{last}"
    );
    // the command to paste stands alone on the line before it
    let lines: Vec<&str> = o.stdout.lines().collect();
    let cmd = lines[lines.len() - 2];
    assert!(
        cmd.starts_with("cd '") && cmd.ends_with("(persistent).'"),
        "{cmd}"
    );
    assert_eq!(
        lines[lines.len() - 3],
        "agent add: next step: start the runtime in the work dir:"
    );
}

// ---- the headless-coordinator form ----

/// An interactive coordinator dir (config and key) from the fake.
fn coordinator(s: &Setup) -> crate::support::board::Agent {
    s.fake.agent("codex@fake-c", Role::Coordinator)
}

fn headless_args(s: &Setup, from: &Path, to: &str) -> Vec<String> {
    [
        "agent",
        "add",
        "--role",
        "headless-coordinator",
        "--from",
        &from.display().to_string(),
        "--agent-dir",
        &s.path(to).display().to_string(),
        "--env-file",
        &s.env_file.display().to_string(),
    ]
    .map(String::from)
    .to_vec()
}

#[test]
fn b8_headless_dir_toml_first_line_key_600_and_no_key_printed() {
    let s = Setup::new();
    let c = coordinator(&s);
    let key_path = c.dir.join("keys/codex@fake-c");
    let key = std::fs::read_to_string(&key_path).unwrap();
    let o = s.run(&headless_args(&s, &c.dir, "headless"), &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    let n = o
        .stdout
        .lines()
        .filter(|l| l.starts_with("agent add: [") && l.contains("/3] ") && l.contains(": done"))
        .count();
    assert_eq!(n, 3, "{}", o.stdout);
    let h = s.path("headless");
    let toml = std::fs::read_to_string(h.join("fake.toml")).unwrap();
    assert_eq!(toml.lines().next(), Some("headless = true"));
    assert!(
        toml.lines().any(|l| l == "name = \"codex@fake-c\""),
        "{toml}"
    );
    assert_eq!(mode(&h.join("fake.toml")), 0o600);
    assert_eq!(mode(&h.join("keys")), 0o700);
    assert_eq!(mode(&h.join("keys/codex@fake-c")), 0o600);
    assert_eq!(
        std::fs::read_to_string(h.join("keys/codex@fake-c")).unwrap(),
        key
    );
    assert_eq!(std::fs::read_link(h.join("agent.env")).unwrap(), s.env_file);
    // the interactive config stays without the key
    assert!(
        !std::fs::read_to_string(c.dir.join("fake.toml"))
            .unwrap()
            .contains("headless")
    );
    // no key line and no token in the output
    for l in key.lines().filter(|l| l.len() > 12) {
        assert!(!o.all().contains(l), "a line of the key was printed");
    }
    assert!(!o.all().contains(TOKEN), "{}", o.all());
    let last = o.stdout.lines().last().unwrap();
    assert!(
        last.starts_with("agent add: next step: install the tick"),
        "{last}"
    );
    assert!(last.contains("--tick --agent-dir "), "{last}");
    assert!(
        last.contains(&format!(
            "--agent-dir '{}' --initiative fake --env-file",
            h.display()
        )),
        "{last}"
    );
}

#[test]
fn b8c_headless_rerun_is_three_times_already_done_with_one_headless_line() {
    let s = Setup::new();
    let c = coordinator(&s);
    assert_eq!(s.run(&headless_args(&s, &c.dir, "headless"), &[]).code, 0);
    let o = s.run(&headless_args(&s, &c.dir, "headless"), &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    let n = o
        .stdout
        .lines()
        .filter(|l| l.contains("/3] ") && l.contains(": already done"))
        .count();
    assert_eq!(n, 3, "{}", o.stdout);
    let toml = std::fs::read_to_string(s.path("headless/fake.toml")).unwrap();
    assert_eq!(
        toml.lines().filter(|l| l.starts_with("headless")).count(),
        1,
        "{toml}"
    );
}

#[test]
fn b9_headless_refusals_leave_no_half_made_dir() {
    let s = Setup::new();
    let c = coordinator(&s);
    assert_eq!(s.run(&headless_args(&s, &c.dir, "headless"), &[]).code, 0);
    // the source is headless itself
    let o = s.run(&headless_args(&s, &s.path("headless"), "h2"), &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("is already headless"), "{}", o.all());
    // no key file
    let nokey = s.path("nokey");
    std::fs::create_dir_all(&nokey).unwrap();
    std::fs::copy(c.dir.join("fake.toml"), nokey.join("fake.toml")).unwrap();
    let o = s.run(&headless_args(&s, &nokey, "h3"), &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("no key file"), "{}", o.all());
    // the same dir
    let mut a = headless_args(&s, &c.dir, "x");
    a[7] = c.dir.display().to_string();
    let o = s.run(&a, &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("is the interactive dir"), "{}", o.all());
    // a dev option
    let mut a = headless_args(&s, &c.dir, "h4");
    a.extend(["--agent".to_string(), "x@y".into()]);
    let o = s.run(&a, &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr
            .contains("takes only --from, --agent-dir and --env-file"),
        "{}",
        o.all()
    );
    // a missing --from
    let o = s.run(
        &["agent", "add", "--role", "headless-coordinator"].map(String::from),
        &[],
    );
    assert_eq!(o.code, 4, "{}", o.all());
    for d in ["h2", "h3", "h4"] {
        assert!(!s.path(d).exists(), "{d} was made");
    }
    // a headless dir whose config is not headless stays and is refused before any write
    let h5 = s.path("h5");
    std::fs::create_dir_all(&h5).unwrap();
    std::fs::write(h5.join("fake.toml"), "name = \"codex@fake-c\"\n").unwrap();
    let o = s.run(&headless_args(&s, &c.dir, "h5"), &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr.contains("exists and is not headless"),
        "{}",
        o.all()
    );
    assert!(!h5.join("agent.env").exists() && !h5.join("keys").exists());
}

// ---- the headless guard ----

#[test]
fn a_headless_config_dir_refuses_agent_add_and_agent_shim() {
    let s = Setup::new();
    let c = coordinator(&s);
    let p = c.dir.join("fake.toml");
    let text = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, format!("headless = true\n{text}")).unwrap();
    let dir = s.path("agents/new").display().to_string();
    for args in [
        vec![
            "agent",
            "add",
            REPO,
            "--agent-dir",
            &dir,
            "--runtime",
            "claude",
            "--initiative",
            "fake",
            "--env-file",
            &s.env_file.display().to_string(),
        ],
        vec!["agent", "shim", "--agent-dir", &dir],
    ] {
        let mut a = vec!["--config-dir".to_string(), c.dir.display().to_string()];
        a.extend(args.iter().map(|x| x.to_string()));
        let o = s.run(&a, &[]);
        assert_eq!(o.code, 2, "{}", o.all());
        assert!(
            o.stderr.contains("headless-scope") && o.stderr.contains("owner or setup command"),
            "{}",
            o.all()
        );
    }
    assert!(!s.path("agents/new").exists());
    assert!(s.fake.requests().is_empty());
    // twin: the same command in the interactive config dir is not refused by the guard
    let o = s.run(
        &[
            "--config-dir".to_string(),
            s.path("plain").display().to_string(),
            "agent".into(),
            "shim".into(),
            "--agent-dir".into(),
            dir.clone(),
        ],
        &[],
    );
    assert!(!o.stderr.contains("headless-scope"), "{}", o.all());
}

#[test]
fn git_and_hostname_get_no_token_variable_from_the_caller() {
    let s = Setup::new();
    let (glog, hlog) = (s.path("git-env"), s.path("hostname-env"));
    write_exe(
        &s.path("fakebin/git"),
        &format!(
            "#!/bin/sh\nenv >> '{}'\nexec /usr/bin/git \"$@\"\n",
            glog.display()
        ),
    );
    write_exe(
        &s.path("fakebin/hostname"),
        &format!("#!/bin/sh\nenv >> '{}'\necho fake-host\n", hlog.display()),
    );
    // no --agent: the host name is asked for
    let mut a = s.args("agents/dev1", "work");
    let i = a.iter().position(|x| x == "--agent").unwrap();
    a.drain(i..i + 2);
    let path = format!(
        "{}:{}:/usr/bin:/bin",
        s.path("fakebin").display(),
        s.realgh.display()
    );
    let o = s.run(
        &a,
        &[
            ("PATH", &path),
            ("GITHUB_TOKEN", "ghp_caller_owner_token2"),
            ("GH_ENTERPRISE_TOKEN", "ghe_caller3"),
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    for (name, f) in [("git", &glog), ("hostname", &hlog)] {
        let seen =
            std::fs::read_to_string(f).unwrap_or_else(|_| panic!("the fake {name} did not run"));
        assert!(seen.contains("PATH="), "{name}: {seen}");
        assert!(
            !seen.contains("TOKEN"),
            "a token variable reached {name}: {seen}"
        );
    }
}

#[test]
fn token_stdin_writes_the_env_file_first_and_the_steps_use_it() {
    let s = Setup::new();
    // a host that has no env file yet: the token arrives on standard input
    std::fs::remove_file(&s.env_file).unwrap();
    let mut a = s.args("agents/dev1", "work");
    a.push("--token-stdin".into());
    let o = s.run_stdin(&a, &[], Some("ghp_pipedHostBToken123456\n"));
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .contains("agent add: the token belongs to fake-owner"),
        "{}",
        o.stdout
    );
    assert!(!o.all().contains("pipedHostBToken"), "{}", o.all());
    assert_eq!(mode(&s.env_file), 0o600);
    assert_eq!(
        std::fs::read_to_string(&s.env_file).unwrap(),
        "GH_TOKEN=ghp_pipedHostBToken123456\n"
    );
    assert_eq!(steps(&o, "done"), 6, "{}", o.stdout);
    // red proof: no token on standard input, no steps
    let o = s.run_stdin(&s.args("agents/dev2", "work2"), &[], None);
    assert_ne!(o.code, 0);
    let mut b = s.args("agents/dev3", "work3");
    b.push("--token-stdin".into());
    let o = s.run_stdin(&b, &[], Some("\n"));
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(!s.path("agents/dev3").exists());
}

#[test]
fn token_stdin_takes_the_workflow_scope_only_in_trial_mode() {
    let s = Setup::new();
    s.fake.set_scopes("project, repo, workflow");
    std::fs::remove_file(&s.env_file).unwrap();
    let mut a = s.args("agents/dev1", "work");
    a.push("--token-stdin".into());
    let o = s.run_stdin(&a, &[], Some("ghp_ownerToken123456\n"));
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(!s.env_file.exists());
    assert!(!s.path("agents/dev1").exists());
    a.push("--insecure-one-account".into());
    let o = s.run_stdin(&a, &[], Some("ghp_ownerToken123456\n"));
    assert!(o.stdout.contains("trial mode"), "{}", o.all());
    assert!(!o.all().contains("ownerToken"), "{}", o.all());
    assert!(s.env_file.exists());
}

#[test]
fn token_stdin_needs_an_initiative_or_an_env_file() {
    let s = Setup::new();
    let mut a = s.args("agents/dev1", "work");
    let i = a.iter().position(|x| x == "--env-file").unwrap();
    a.drain(i..i + 2);
    a.push("--token-stdin".into());
    let o = s.run_stdin(&a, &[], Some("ghp_pipedHostBToken123456\n"));
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr.contains("--token-stdin needs --initiative"),
        "{}",
        o.all()
    );
}

#[test]
fn wait_approval_prints_the_owner_line_and_goes_on_when_the_approval_comes() {
    let s = Setup::new();
    let mut a = s.args("agents/dev1", "work");
    a.extend(["--wait-approval", "--wait-interval", "1"].map(String::from));
    let o = std::thread::scope(|sc| {
        let run = sc.spawn(|| s.run(&a, &[]));
        // the owner approves as soon as the join is posted (the agent config is written then)
        let cfg = s.path("agents/dev1/fake.toml");
        let t = Instant::now();
        while !cfg.exists() {
            assert!(t.elapsed() < Duration::from_secs(60), "no join");
            std::thread::sleep(Duration::from_millis(50));
        }
        // the join request is posted after the config is written
        let t = Instant::now();
        while !s
            .fake
            .requests()
            .iter()
            .any(|r| r.method == "POST" && r.path.contains("/comments"))
        {
            assert!(t.elapsed() < Duration::from_secs(60), "no join request");
            std::thread::sleep(Duration::from_millis(50));
        }
        s.approve(NAME, &[]);
        run.join().unwrap()
    });
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains(&format!(
            "agent add: owner: run this on the owner machine: crewboard approve {NAME} --initiative fake"
        )),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("[7/7] start: waiting:"), "{}", o.stdout);
    assert!(
        o.stdout
            .contains(&format!("[7/7] start: done ({NAME} is active)")),
        "{}",
        o.stdout
    );
    assert!(!o.stdout.contains("not yet"), "{}", o.stdout);
}

#[test]
fn wait_approval_gives_up_after_the_longest_wait_and_a_rerun_without_it_is_unchanged() {
    let s = Setup::new();
    let mut a = s.args("agents/dev1", "work");
    a.extend(["--wait-approval", "--wait-interval", "1", "--wait-max", "2"].map(String::from));
    let o = s.run(&a, &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("gave up after 0 min"), "{}", o.stdout);
    assert!(
        o.stdout
            .contains("[7/7] start: not yet: the owner has not approved"),
        "{}",
        o.stdout
    );
    // without the flag the same state ends as before: not yet, no waiting
    let o = s.add("agents/dev1", "work", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!o.stdout.contains("waiting"), "{}", o.stdout);
    assert!(o.stdout.contains("start: not yet"), "{}", o.stdout);
}

// ---------------------------------------------------------------------------------------------
// --install-wake: the unit of `wake install`, in the same process, against fake scheduler programs

/// Fake `systemctl`, `launchctl`, `loginctl`, `crontab` and a `claude`, on a PATH of their own:
/// no test touches a real scheduler. Every call of a scheduler program goes to `$RIG/calls`.
struct Sched {
    dir: PathBuf,
}

impl Sched {
    fn new(s: &Setup) -> Sched {
        let dir = s.path("sched");
        for c in ["systemctl", "launchctl"] {
            write_exe(
                &dir.join(c),
                &format!(
                    "#!/bin/bash\necho \"{c} $*\" >>\"$RIG/calls\"\ncase \"$*\" in *is-enabled*) echo enabled;; esac\nexit 0\n"
                ),
            );
        }
        write_exe(
            &dir.join("loginctl"),
            "#!/bin/bash\necho \"loginctl $*\" >>\"$RIG/calls\"\nif [ -f \"$RIG/linger\" ]; then echo Linger=yes; else echo Linger=no; fi\n",
        );
        write_exe(
            &dir.join("crontab"),
            "#!/bin/bash\necho \"crontab $*\" >>\"$RIG/calls\"\nif [ \"$1\" = -l ]; then [ -f \"$RIG/crontab\" ] || exit 1; cat \"$RIG/crontab\"; else cat >\"$RIG/crontab.new\"; mv \"$RIG/crontab.new\" \"$RIG/crontab\"; fi\n",
        );
        write_exe(&dir.join("claude"), "#!/bin/sh\nexit 0\n");
        Sched { dir }
    }

    /// `agent add` with the fake programs first on the PATH; `os` plays the operating system.
    fn add(&self, s: &Setup, os: &str, extra: &[&str]) -> Out {
        let mut a = s.args("agents/dev1", "work");
        a.extend(extra.iter().map(|x| x.to_string()));
        let path = format!(
            "{}:{}:/usr/bin:/bin",
            self.dir.display(),
            s.realgh.display()
        );
        let rig = s.tmp.path().display().to_string();
        s.run(
            &a,
            &[
                ("PATH", &path),
                ("RIG", &rig),
                ("USER", "wf"),
                ("CREWBOARD_WAKE_OS", os),
            ],
        )
    }

    /// The commands of the scheduler that ran, without the read-only `crontab -l`.
    fn calls(&self, s: &Setup) -> Vec<String> {
        std::fs::read_to_string(s.path("calls"))
            .unwrap_or_default()
            .lines()
            .filter(|l| *l != "crontab -l")
            .map(str::to_string)
            .collect()
    }
}

#[test]
fn install_wake_installs_the_unit_after_start_and_ends_on_the_next_line_of_wake_install() {
    let s = Setup::new();
    let sc = Sched::new(&s);
    assert_eq!(s.add("agents/dev1", "work", &[]).code, 0);
    s.approve(NAME, &[]);
    let o = sc.add(&s, "Linux", &["--install-wake", "--every", "10m"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let lines: Vec<&str> = o.stdout.lines().collect();
    let at = |p: &str| lines.iter().position(|l| l.starts_with(p));
    let started = at("agent add: [7/7] start: done").expect("start line");
    let status = at("agent add: wake: installed crewboard-fake-").expect("status line");
    assert!(
        o.stdout.contains("(systemd, every 10m); log: "),
        "{}",
        o.all()
    );
    let next = at("agent add: wake: next: loginctl enable-linger").expect("next line");
    assert!(started < status && status < next, "{}", o.stdout);
    // one next step, and it is the command of the `next:` line
    assert_eq!(next_steps(&o), 1, "{}", o.stdout);
    let last = lines.last().unwrap();
    assert!(
        last.starts_with("agent add: next step: loginctl enable-linger \"$USER\" "),
        "{last}"
    );
    // the unit is on disk and the fake systemd was asked, the real one never
    let units = s.path("home/.config/systemd/user");
    let names: Vec<String> = std::fs::read_dir(&units)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    let calls = sc.calls(&s);
    assert!(
        calls.contains(&"systemctl --user daemon-reload".to_string()),
        "{calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.contains("enable crewboard-fake-")),
        "{calls:?}"
    );
    // no token in the unit files or the wrapper
    let all = all_text(&s.path("agents/dev1")) + &all_text(&units);
    assert!(!all.contains(TOKEN), "token in a file");
    // a second run changes nothing and says so again
    let again = sc.add(&s, "Linux", &["--install-wake", "--every", "10m"]);
    assert_eq!(again.code, 0, "{}", again.all());
    assert_eq!(std::fs::read_dir(&units).unwrap().count(), 2);
}

#[test]
fn install_wake_with_linger_on_or_on_a_mac_has_no_next_command() {
    let s = Setup::new();
    let sc = Sched::new(&s);
    std::fs::write(s.path("linger"), "").unwrap();
    assert_eq!(s.add("agents/dev1", "work", &[]).code, 0);
    s.approve(NAME, &[]);
    let o = sc.add(&s, "Linux", &["--install-wake"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!o.stdout.contains("wake: next:"), "{}", o.stdout);
    let last = o.stdout.lines().last().unwrap();
    assert!(
        last.starts_with(&format!(
            "agent add: next step: none; the scheduler wakes {NAME} when work exists (check: crewboard wake status --agent-dir '"
        )),
        "{last}"
    );
    // launchd on a Mac: the plist, no linger
    let m = Setup::new();
    let mc = Sched::new(&m);
    assert_eq!(m.add("agents/dev1", "work", &[]).code, 0);
    m.approve(NAME, &[]);
    let o = mc.add(&m, "Darwin", &["--install-wake", "--scheduler", "launchd"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("(launchd, every 5m)"), "{}", o.stdout);
    assert!(!o.stdout.contains("wake: next:"), "{}", o.stdout);
    let plists = m.path("home/Library/LaunchAgents");
    assert_eq!(std::fs::read_dir(plists).unwrap().count(), 1);
}

#[test]
fn install_wake_passes_allow_to_the_wake_and_the_rerun_line_keeps_it() {
    let s = Setup::new();
    let sc = Sched::new(&s);
    // before the approval: nothing is installed, the rerun line names the tool
    let o = sc.add(
        &s,
        "Linux",
        &[
            "--install-wake",
            "--scheduler",
            "cron",
            "--allow",
            "Bash(make:*)",
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    let last = o.stdout.lines().last().unwrap();
    assert!(last.ends_with("--allow 'Bash(make:*)'"), "{last}");
    s.approve(NAME, &[]);
    let o = sc.add(
        &s,
        "Linux",
        &[
            "--install-wake",
            "--scheduler",
            "cron",
            "--allow",
            "Bash(make:*)",
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    let all = all_text(&s.path("agents/dev1")) + &all_text(&s.path("home"));
    assert!(
        all.contains("Bash(make:*)"),
        "the tool is not in the wake files"
    );
    // `--allow` alone is refused, as `--every` is
    let o = sc.add(&s, "Linux", &["--allow", "Bash(make:*)"]);
    assert_ne!(o.code, 0, "{}", o.all());
}

#[test]
fn install_wake_waits_for_the_approval_and_installs_nothing_before_it() {
    let s = Setup::new();
    let sc = Sched::new(&s);
    let o = sc.add(&s, "Linux", &["--install-wake", "--scheduler", "cron"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("start: not yet"), "{}", o.stdout);
    // the rerun line keeps the flags, and nothing of the scheduler ran or was written
    let last = o.stdout.lines().last().unwrap();
    assert!(last.ends_with("--install-wake --scheduler cron"), "{last}");
    assert!(sc.calls(&s).is_empty(), "{:?}", sc.calls(&s));
    assert!(!s.path("home/.config/systemd").exists());
    assert!(!o.stdout.contains("wake:"), "{}", o.stdout);
}

#[test]
fn install_wake_keeps_the_refusal_for_another_job_and_its_fix_line() {
    let s = Setup::new();
    let sc = Sched::new(&s);
    assert_eq!(s.add("agents/dev1", "work", &[]).code, 0);
    s.approve(NAME, &[]);
    let dir = s.path("agents/dev1");
    std::fs::write(
        s.path("crontab"),
        format!(
            "0 3 * * * /usr/bin/true\n*/10 * * * * crewboard poll --config-dir {}\n",
            dir.display()
        ),
    )
    .unwrap();
    let o = sc.add(&s, "Linux", &["--install-wake"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains(
            "agent add: wake install: refused [rule=second-wake-job]: another wake job runs this agent: cron */10 * * * * crewboard poll"
        ),
        "{}",
        o.all()
    );
    // the fix line of `wake install` is shown, and --replace is not passed on its own
    assert!(
        o.stderr
            .contains("wake install: fix: crewboard wake install --agent-dir "),
        "{}",
        o.all()
    );
    assert!(o.stderr.contains("--replace"), "{}", o.all());
    assert!(
        o.stderr.contains("only the scheduler unit is missing"),
        "{}",
        o.all()
    );
    assert!(o.stdout.contains("start: done"), "{}", o.stdout);
    assert!(!s.path("home/.config/systemd").exists());
    assert_eq!(
        std::fs::read_to_string(s.path("crontab"))
            .unwrap()
            .matches("crewboard poll")
            .count(),
        1,
        "the other job is untouched"
    );
    // the scheduler was only probed: nothing was enabled, reloaded or written
    let calls = sc.calls(&s);
    assert!(
        calls
            .iter()
            .all(|c| c.contains("show-environment") || c.contains("is-")),
        "{calls:?}"
    );
}

#[test]
fn install_wake_usage_errors_exit_4_or_2_and_write_nothing() {
    let s = Setup::new();
    let sc = Sched::new(&s);
    // a bad interval is refused before the first step
    let o = sc.add(&s, "Linux", &["--install-wake", "--every", "soon"]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("--every needs a duration"), "{}", o.all());
    assert!(!s.path("agents/dev1").exists(), "nothing was written");
    // the interactive coordinator has no unit
    let o = sc.add(&s, "Linux", &["--install-wake", "--role", "coordinator"]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr.contains("--install-wake is for a dev agent"),
        "{}",
        o.all()
    );
    assert!(!s.path("agents/dev1").exists());
    // --scheduler and --every need the flag; hermes is not a value of --scheduler
    for extra in [vec!["--scheduler", "cron"], vec!["--every", "5m"]] {
        let o = sc.add(&s, "Linux", &extra);
        assert_ne!(o.code, 0, "{}", o.all());
        assert!(o.stderr.contains("--install-wake"), "{}", o.all());
    }
    let o = sc.add(&s, "Linux", &["--install-wake", "--scheduler", "hermes"]);
    assert_ne!(o.code, 0, "{}", o.all());
    assert!(!s.path("agents/dev1").exists());
}
