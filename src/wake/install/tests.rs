//! The plan of `wake install` against a fake host. The expected unit texts are golden: the
//! names, the paths and every line of a unit must not change, or an earlier install would not be
//! found again.

use super::foreign::{find_foreign, replace_plan};
use super::plan::*;
use super::sys::{Ran, Sys};
use super::units::{self, fill};
use super::{Os, Scheduler};
use crate::wake::job::AgentArgs;
use crate::wake::settings::WakeArgs;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Default)]
struct Fake {
    exists: HashSet<PathBuf>,
    dirs: HashSet<PathBuf>,
    execs: HashSet<PathBuf>,
    files: HashMap<PathBuf, String>,
    answers: HashMap<String, Ran>,
    runs: RefCell<Vec<String>>,
}

impl Sys for Fake {
    fn exists(&self, p: &Path) -> bool {
        self.exists.contains(p) || self.files.contains_key(p)
    }
    fn read_text(&self, p: &Path) -> Option<String> {
        self.files.get(p).cloned()
    }
    fn list(&self, dir: &Path) -> Vec<PathBuf> {
        let mut v: Vec<_> = self
            .files
            .keys()
            .filter(|k| k.parent() == Some(dir))
            .cloned()
            .collect();
        v.sort();
        v
    }
    fn is_dir(&self, p: &Path) -> bool {
        self.dirs.contains(p)
    }
    fn is_executable(&self, p: &Path) -> bool {
        self.execs.contains(p)
    }
    fn run(&self, program: &str, args: &[String], _stdin: Option<&str>) -> Ran {
        let line = format!("{program} {}", args.join(" "));
        self.runs.borrow_mut().push(line.clone());
        self.answers.get(&line).cloned().unwrap_or_default()
    }
}

fn host(os: Os) -> Host {
    Host {
        home: "/h".into(),
        xdg_config: None,
        path: "/usr/bin:/bin".into(),
        os,
        user: Some("wf".into()),
        uid: "501".into(),
    }
}

fn fake() -> Fake {
    let mut f = Fake::default();
    f.dirs.insert("/w".into());
    for r in ["codex", "claude", "hermes"] {
        f.execs.insert(format!("/nvm/{r}").into());
    }
    f.execs.insert("/cb/crewboard".into());
    f
}

fn target(dir: &str) -> Target {
    Target {
        config_dir: format!("/h/.config/crewboard/agents/{dir}").into(),
        initiative: "demo".into(),
        agent: Some("dev@host-a".into()),
    }
}

fn dev(runtime: &str) -> WakeArgs {
    WakeArgs {
        agent_job: AgentArgs {
            runtime: Some(runtime.into()),
            work_dir: Some("/w".into()),
            runtime_bin: Some(format!("/nvm/{runtime}").into()),
            ..AgentArgs::default()
        },
        ..WakeArgs::default()
    }
}

fn make(
    h: &Host,
    sys: &Fake,
    run: &WakeArgs,
    t: &Target,
    sched: Scheduler,
    secs: u64,
) -> Result<InstallPlan, String> {
    plan_install(
        h,
        sys,
        &InstallIn {
            run,
            scheduler: sched,
            secs,
            target: t,
            env_file: Some(Path::new("/e/demo.env")),
            state_dir: Path::new("/s/crewboard/agents/demo-dev1"),
            log: Path::new("/s/crewboard/agents/demo-dev1/wake.log"),
            crewboard: Path::new("/cb/crewboard"),
            ts: "2026-10-25T10:00:00Z",
        },
    )
}

fn content<'a>(p: &'a InstallPlan, suffix: &str) -> &'a str {
    &p.files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .unwrap_or_else(|| panic!("no file {suffix}: {:?}", p.files))
        .content
}

#[test]
fn systemd_service_and_timer_are_the_texts_of_the_shell_install() {
    let p = make(
        &host(Os::Linux),
        &fake(),
        &dev("codex"),
        &target("dev1"),
        Scheduler::Systemd,
        600,
    )
    .unwrap();
    assert_eq!(p.name, "crewboard-demo-dev1");
    assert_eq!(
        content(&p, "crewboard-demo-dev1.service"),
        "[Unit]\nDescription=crewboard wake for dev@host-a in demo\n\n[Service]\nType=oneshot\nExecStart=/bin/bash /h/.config/crewboard/agents/dev1/crewboard-demo-dev1-wake.sh\nTimeoutStartSec=60min\n"
    );
    assert_eq!(
        content(&p, "crewboard-demo-dev1.timer"),
        "[Unit]\nDescription=crewboard wake timer for dev@host-a in demo\n\n[Timer]\nOnBootSec=2min\nOnUnitActiveSec=600s\n\n[Install]\nWantedBy=timers.target\n"
    );
    let paths: Vec<_> = p
        .files
        .iter()
        .map(|f| f.path.display().to_string())
        .collect();
    assert_eq!(
        paths,
        [
            "/h/.config/crewboard/agents/dev1/crewboard-demo-dev1-wake.sh",
            "/h/.config/systemd/user/crewboard-demo-dev1.service",
            "/h/.config/systemd/user/crewboard-demo-dev1.timer",
        ]
    );
    assert_eq!(p.files[0].mode, 0o700);
    let cmds: Vec<_> = p.cmds.iter().map(Cmd::line).collect();
    assert_eq!(
        cmds,
        [
            "systemctl --user daemon-reload",
            "systemctl --user enable crewboard-demo-dev1.timer",
            "systemctl --user restart crewboard-demo-dev1.timer",
        ]
    );
    assert!(p.linger_check);
}

#[test]
fn xdg_config_home_moves_the_systemd_dir() {
    let mut h = host(Os::Linux);
    h.xdg_config = Some("/x/cfg".into());
    let p = make(
        &h,
        &fake(),
        &dev("codex"),
        &target("dev1"),
        Scheduler::Systemd,
        300,
    )
    .unwrap();
    assert!(
        p.files
            .iter()
            .any(|f| f.path == Path::new("/x/cfg/systemd/user/crewboard-demo-dev1.timer"))
    );
}

#[test]
fn launchd_plist_is_the_text_of_the_shell_install() {
    let p = make(
        &host(Os::Mac),
        &fake(),
        &dev("claude"),
        &target("dev1"),
        Scheduler::Launchd,
        300,
    )
    .unwrap();
    let want = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>crewboard-demo-dev1</string>\n  <key>ProgramArguments</key>\n  <array><string>/bin/bash</string><string>/h/.config/crewboard/agents/dev1/crewboard-demo-dev1-wake.sh</string></array>\n  <key>StartInterval</key><integer>300</integer>\n  <key>StandardOutPath</key><string>/s/crewboard/agents/demo-dev1/scheduler.err</string>\n  <key>StandardErrorPath</key><string>/s/crewboard/agents/demo-dev1/scheduler.err</string>\n</dict>\n</plist>\n";
    assert_eq!(content(&p, "crewboard-demo-dev1.plist"), want);
    assert_eq!(
        p.files[1].path,
        Path::new("/h/Library/LaunchAgents/crewboard-demo-dev1.plist")
    );
    let cmds: Vec<_> = p.cmds.iter().map(Cmd::line).collect();
    assert_eq!(
        cmds,
        [
            "launchctl bootout gui/501/crewboard-demo-dev1",
            "launchctl bootstrap gui/501 /h/Library/LaunchAgents/crewboard-demo-dev1.plist",
        ]
    );
    assert!(p.cmds[0].ignore_failure && !p.cmds[1].ignore_failure);
    assert!(p.next.is_none());
}

#[test]
fn a_dir_with_a_space_and_an_ampersand_is_quoted_per_format() {
    let t = target("dev one&2");
    let sys = fake();
    let s = make(
        &host(Os::Linux),
        &sys,
        &dev("codex"),
        &t,
        Scheduler::Systemd,
        300,
    )
    .unwrap();
    assert_eq!(s.name, "crewboard-demo-dev_one_2");
    assert!(content(&s, ".service").contains(
        "ExecStart=/bin/bash \"/h/.config/crewboard/agents/dev one&2/crewboard-demo-dev_one_2-wake.sh\"\n"
    ));
    let l = make(
        &host(Os::Mac),
        &sys,
        &dev("codex"),
        &t,
        Scheduler::Launchd,
        300,
    )
    .unwrap();
    assert!(content(&l, ".plist").contains(
        "<string>/h/.config/crewboard/agents/dev one&amp;2/crewboard-demo-dev_one_2-wake.sh</string>"
    ));
    let c = make(
        &host(Os::Linux),
        &sys,
        &dev("codex"),
        &t,
        Scheduler::Cron,
        300,
    )
    .unwrap();
    assert!(content(&c, ".cron").contains(
        "/bin/bash '/h/.config/crewboard/agents/dev one&2/crewboard-demo-dev_one_2-wake.sh' >>'/s/crewboard/agents/demo-dev1/scheduler.err' 2>&1 "
    ));
}

#[test]
fn cron_line_rounds_the_interval_up_to_minutes_and_has_the_marker() {
    let mut sys = fake();
    sys.answers.insert(
        "crontab -l".into(),
        Ran {
            ok: true,
            stdout: "0 3 * * * /usr/bin/true\n*/5 * * * * x # crewboard:crewboard-demo-dev1\n"
                .into(),
            stderr: String::new(),
        },
    );
    let p = make(
        &host(Os::Linux),
        &sys,
        &dev("codex"),
        &target("dev1"),
        Scheduler::Cron,
        420,
    )
    .unwrap();
    let line = "*/7 * * * * /bin/bash '/h/.config/crewboard/agents/dev1/crewboard-demo-dev1-wake.sh' >>'/s/crewboard/agents/demo-dev1/scheduler.err' 2>&1 # crewboard:crewboard-demo-dev1\n";
    assert_eq!(content(&p, "crewboard-demo-dev1.cron"), line);
    assert_eq!(p.cmds.len(), 1);
    // the old line of this unit goes, the other lines stay, the new line is last
    assert_eq!(
        p.cmds[0].stdin.as_deref(),
        Some(format!("0 3 * * * /usr/bin/true\n{line}").as_str())
    );
    let ninety = make(
        &host(Os::Linux),
        &fake(),
        &dev("codex"),
        &target("dev1"),
        Scheduler::Cron,
        90,
    )
    .unwrap();
    assert!(content(&ninety, ".cron").starts_with("*/2 * * * * "));
}

#[test]
fn hermes_puts_the_wrapper_in_the_hermes_dir_and_prints_the_cron_create() {
    let p = make(
        &host(Os::Linux),
        &fake(),
        &dev("hermes"),
        &target("dev1"),
        Scheduler::Hermes,
        300,
    )
    .unwrap();
    assert_eq!(
        p.files[0].path,
        Path::new("/h/.hermes/scripts/crewboard-demo-dev1-wake.sh")
    );
    assert_eq!(p.files.len(), 1);
    assert!(p.cmds.is_empty());
    assert_eq!(
        p.next.as_deref(),
        Some(
            "hermes cron create --name crewboard-demo-dev1 --no-agent --script crewboard-demo-dev1-wake.sh --deliver local --failure-deliver origin \"every 5m\""
        )
    );
}

#[test]
fn the_wrapper_stores_the_path_and_the_absolute_paths_and_runs_crewboard_wake() {
    let mut run = dev("claude");
    run.agent_job.model = Some("m 1".into());
    run.agent_job.allow = vec!["Bash(npm *)".into(), "Bash(make *)".into()];
    run.agent_job.events_every = Some("120".into());
    run.agent_job.host_lock = vec!["/hl".into()];
    run.max_wake = Some("30m".into());
    let p = make(
        &host(Os::Mac),
        &fake(),
        &run,
        &target("dev1"),
        Scheduler::Launchd,
        300,
    )
    .unwrap();
    assert_eq!(
        p.files[0].content,
        "#!/bin/bash\n# Written by crewboard wake install (launchd) on 2026-10-25T10:00:00Z. Run it again to change it.\n# PATH: the PATH of the installing shell, so that the model finds the build tools.\nexport PATH='/usr/bin:/bin'\nexec '/cb/crewboard' 'wake' '--agent' 'dev@host-a' '--initiative' 'demo' '--env-file' '/e/demo.env' '--config-dir' '/h/.config/crewboard/agents/dev1' '--runtime' 'claude' '--work-dir' '/w' '--allow' 'Bash(npm *)' '--allow' 'Bash(make *)' '--model' 'm 1' '--host-lock' '/hl' '--events-every' '120' '--runtime-bin' '/nvm/claude' '--max-wake' '30m' \"$@\"\n"
    );
}

#[test]
fn a_coordinator_runs_the_tick_without_a_runtime() {
    let mut t = target("demo-coord-headless");
    t.agent = None;
    let run = WakeArgs {
        tick: true,
        ..WakeArgs::default()
    };
    let p = make(&host(Os::Linux), &fake(), &run, &t, Scheduler::Systemd, 300).unwrap();
    assert_eq!(p.name, "crewboard-demo-coord-headless");
    assert!(p.files[0].content.ends_with(
        "exec '/cb/crewboard' 'wake' '--tick' '--initiative' 'demo' '--env-file' '/e/demo.env' '--config-dir' '/h/.config/crewboard/agents/demo-coord-headless' \"$@\"\n"
    ));
    assert!(
        content(&p, ".service")
            .contains("Description=crewboard wake for the coordinator in demo\n")
    );
    assert!(
        content(&p, ".timer")
            .contains("Description=crewboard wake timer for the coordinator in demo\n")
    );
}

#[test]
fn a_unit_of_an_earlier_version_with_the_doubled_name_is_updated_in_place() {
    let mut sys = fake();
    sys.exists
        .insert("/h/.config/systemd/user/crewboard-demo-demo-claude.timer".into());
    let t = target("demo-claude");
    let p = make(
        &host(Os::Linux),
        &sys,
        &dev("codex"),
        &t,
        Scheduler::Systemd,
        300,
    )
    .unwrap();
    assert_eq!(p.name, "crewboard-demo-demo-claude");
    assert!(
        p.note
            .as_deref()
            .unwrap()
            .starts_with("note: keeping the installed name crewboard-demo-demo-claude")
    );
    assert!(
        p.files
            .iter()
            .all(|f| !f.path.to_string_lossy().contains("crewboard-demo-claude"))
    );
    // without an old unit the short name is used
    let q = make(
        &host(Os::Linux),
        &fake(),
        &dev("codex"),
        &t,
        Scheduler::Systemd,
        300,
    )
    .unwrap();
    assert_eq!(q.name, "crewboard-demo-claude");
    assert!(q.note.is_none());
}

#[test]
fn names_follow_the_rule_of_the_shell_script() {
    let n = names("demo", Path::new("/a/dev1"));
    assert_eq!(
        (n.current.as_str(), n.old.as_str()),
        ("crewboard-demo-dev1", "crewboard-demo-dev1")
    );
    let n = names("demo", Path::new("/a/demo-codex/"));
    assert_eq!(n.current, "crewboard-demo-codex");
    assert_eq!(n.old, "crewboard-demo-demo-codex");
    assert_eq!(
        names("d@x", Path::new("/a/b c")).current,
        "crewboard-d_x-b_c"
    );
}

#[test]
fn uninstall_finds_the_short_and_the_doubled_name_and_only_what_exists() {
    let mut sys = fake();
    for p in [
        "/h/.config/systemd/user/crewboard-demo-demo-claude.timer",
        "/h/.config/crewboard/agents/demo-claude/crewboard-demo-demo-claude-wake.sh",
        "/h/.config/systemd/user/crewboard-demo-claude.timer",
    ] {
        sys.exists.insert(p.into());
    }
    let t = target("demo-claude");
    let all = plan_uninstall(&host(Os::Linux), &sys, &t, None);
    let found: Vec<_> = all.iter().map(|r| (r.scheduler, r.name.as_str())).collect();
    assert_eq!(
        found,
        [
            (Scheduler::Systemd, "crewboard-demo-claude"),
            (Scheduler::Systemd, "crewboard-demo-demo-claude")
        ]
    );
    assert_eq!(
        all[0].before[0].line(),
        "systemctl --user disable --now crewboard-demo-claude.timer"
    );
    // a scheduler that is named is cleaned even when nothing is there
    let named = plan_uninstall(&host(Os::Linux), &fake(), &t, Some(Scheduler::Launchd));
    assert_eq!(named.len(), 2);
    assert!(plan_uninstall(&host(Os::Linux), &fake(), &t, None).is_empty());
}

#[test]
fn refusals_name_the_wrong_option() {
    let h = host(Os::Linux);
    let sys = fake();
    let t = target("dev1");
    let go = |run: &WakeArgs, s, h: &Host| make(h, &sys, run, &t, s, 300).unwrap_err();
    assert!(
        go(&dev("codex"), Scheduler::Launchd, &h).contains("launchd is the scheduler of macOS")
    );
    assert!(
        go(&dev("claude"), Scheduler::Cron, &host(Os::Mac)).contains("keychain"),
        "cron on macOS cannot hold the claude login"
    );
    let mut bad = dev("codex");
    bad.agent_job.runtime_bin = Some("/nope/codex".into());
    assert!(go(&bad, Scheduler::Systemd, &h).contains("codex is not on the PATH of this shell"));
    let mut bad = dev("codex");
    bad.agent_job.runtime = Some("vim".into());
    assert!(go(&bad, Scheduler::Systemd, &h).contains("--runtime must be claude, codex or hermes"));
    let mut bad = dev("codex");
    bad.agent_job.work_dir = Some("/missing".into());
    assert!(go(&bad, Scheduler::Systemd, &h).contains("--work-dir must be an existing directory"));
    let mut bad = dev("codex");
    bad.agent_job.allow = vec!["x".into()];
    assert!(go(&bad, Scheduler::Systemd, &h).contains("--allow is for --runtime claude"));
    let mut bad = dev("claude");
    bad.agent_job.events_every = Some("5m".into());
    assert!(go(&bad, Scheduler::Systemd, &h).contains("--events-every needs whole seconds"));
    let mut bad = dev("codex");
    bad.tick = true;
    assert!(go(&bad, Scheduler::Systemd, &h).contains("exclude each other"));
}

#[test]
fn auto_picks_launchd_on_macos_systemd_when_it_works_else_cron_and_hermes_for_hermes() {
    let sys = fake();
    assert_eq!(
        pick_scheduler(Scheduler::Auto, &host(Os::Mac), &sys, Some("claude")),
        Scheduler::Launchd
    );
    assert_eq!(
        pick_scheduler(Scheduler::Auto, &host(Os::Linux), &sys, Some("hermes")),
        Scheduler::Hermes
    );
    // `systemctl --user` does not work: the fake answers not ok
    assert_eq!(
        pick_scheduler(Scheduler::Auto, &host(Os::Linux), &sys, Some("codex")),
        Scheduler::Cron
    );
    let mut ok = fake();
    ok.answers.insert(
        "systemctl --user show-environment".into(),
        Ran {
            ok: true,
            ..Ran::default()
        },
    );
    assert_eq!(
        pick_scheduler(Scheduler::Auto, &host(Os::Linux), &ok, Some("codex")),
        Scheduler::Systemd
    );
    assert_eq!(
        pick_scheduler(Scheduler::Cron, &host(Os::Mac), &sys, None),
        Scheduler::Cron
    );
}

#[test]
fn fill_replaces_in_one_pass_and_keeps_unknown_keys() {
    assert_eq!(
        fill("a @X@ b @Y@ c @Z", &[("X", "@Y@"), ("Y", "2")]),
        "a @Y@ b 2 c @Z"
    );
    assert_eq!(fill("no keys @", &[]), "no keys @");
}

#[test]
fn unit_arg_quotes_like_systemd_and_sq_like_a_shell() {
    assert_eq!(units::unit_arg("/a/b-c_d.sh"), "/a/b-c_d.sh");
    assert_eq!(
        units::unit_arg("/a b/%x$y\"z\\"),
        "\"/a b/%%x$$y\\\"z\\\\\""
    );
    assert_eq!(units::unit_arg(""), "\"\"");
    assert_eq!(units::sq("it's"), "'it'\\''s'");
    assert_eq!(units::xml_esc("a&b<c>"), "a&amp;b&lt;c&gt;");
}

const DIR: &str = "/h/.config/crewboard/agents/dev1";

fn with_file(sys: &mut Fake, path: &str, text: &str) {
    sys.files.insert(path.into(), text.to_string());
}

fn plist(label: &str, args: &[&str]) -> String {
    let words: String = args
        .iter()
        .map(|a| format!("<string>{a}</string>"))
        .collect();
    format!(
        "<plist><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array>{words}</array></dict></plist>"
    )
}

#[test]
fn a_launchd_job_of_another_name_that_runs_a_wrapper_for_the_same_dir_is_found() {
    let mut sys = fake();
    // the wrapper script is followed one level
    with_file(
        &mut sys,
        "/h/.config/crewboard/agents/dev1/poll.sh",
        &format!("#!/bin/bash\nexec crewboard poll --config-dir {DIR}\n"),
    );
    with_file(
        &mut sys,
        "/h/Library/LaunchAgents/com.crewboard.demo.dev1.plist",
        &plist(
            "com.crewboard.demo.dev1",
            &["/bin/bash", &format!("{DIR}/poll.sh")],
        ),
    );
    // another agent, the headless dir of this agent, and a job without wake or poll: not found
    with_file(
        &mut sys,
        "/h/Library/LaunchAgents/com.other.plist",
        &plist(
            "com.other",
            &[
                "/bin/bash",
                "-c",
                "crewboard poll --config-dir /h/.config/crewboard/agents/dev2",
            ],
        ),
    );
    with_file(
        &mut sys,
        "/h/Library/LaunchAgents/com.headless.plist",
        &plist(
            "com.headless",
            &[
                "/bin/bash",
                "-c",
                &format!("crewboard wake --tick --config-dir {DIR}-headless"),
            ],
        ),
    );
    with_file(
        &mut sys,
        "/h/Library/LaunchAgents/com.watch.plist",
        &plist("com.watch", &["/bin/bash", "-c", &format!("echo {DIR}")]),
    );
    // our own plist (the current name) is not foreign
    with_file(
        &mut sys,
        "/h/Library/LaunchAgents/crewboard-demo-dev1.plist",
        &plist(
            "crewboard-demo-dev1",
            &["/bin/bash", &format!("{DIR}/crewboard-demo-dev1-wake.sh")],
        ),
    );
    // a backup file is not a job
    with_file(
        &mut sys,
        "/h/Library/LaunchAgents/com.old.plist.bak-20261024",
        &plist(
            "com.old",
            &[
                "/bin/bash",
                "-c",
                &format!("crewboard wake --config-dir {DIR}"),
            ],
        ),
    );
    let found = find_foreign(&host(Os::Mac), &sys, &target("dev1"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(
        found[0].line(),
        "another wake job runs this agent: launchd com.crewboard.demo.dev1 (/h/Library/LaunchAgents/com.crewboard.demo.dev1.plist)"
    );
}

#[test]
fn systemd_cron_and_hermes_jobs_of_other_names_are_found_and_our_own_are_not() {
    let mut sys = fake();
    with_file(
        &mut sys,
        "/h/.config/systemd/user/legacy-wake.service",
        "[Service]\nExecStart=/bin/bash /x/agent-wake.sh --config-dir ~/.config/crewboard/agents/dev1 --runtime codex\n",
    );
    with_file(
        &mut sys,
        "/h/.config/systemd/user/crewboard-demo-dev1.service",
        &format!("[Service]\nExecStart=/bin/bash {DIR}/crewboard-demo-dev1-wake.sh\n"),
    );
    with_file(
        &mut sys,
        "/h/.hermes/scripts/legacy.sh",
        &format!("#!/bin/bash\ncrewboard wake --agent-dir {DIR}\n"),
    );
    with_file(
        &mut sys,
        "/h/.hermes/scripts/crewboard-demo-dev1-wake.sh",
        &format!("crewboard wake --agent-dir {DIR}\n"),
    );
    let own = format!(
        "*/5 * * * * /bin/bash '{DIR}/crewboard-demo-dev1-wake.sh' >>/x 2>&1 # crewboard:crewboard-demo-dev1"
    );
    let foreign = format!("*/10 * * * * crewboard poll --config-dir {DIR} >>/x 2>&1");
    sys.answers.insert(
        "crontab -l".into(),
        Ran {
            ok: true,
            stdout: format!("0 3 * * * /usr/bin/true\n{own}\n{foreign}\n"),
            stderr: String::new(),
        },
    );
    let found = find_foreign(&host(Os::Linux), &sys, &target("dev1"));
    let lines: Vec<String> = found.iter().map(|f| f.line()).collect();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines.iter().any(|l| l == "another wake job runs this agent: systemd legacy-wake (/h/.config/systemd/user/legacy-wake.service)"));
    assert!(lines.iter().any(|l| {
        l.starts_with("another wake job runs this agent: cron */10 * * * * crewboard poll")
            && l.ends_with("(crontab)")
    }));
    assert!(
        lines.iter().any(|l| l
            == "another wake job runs this agent: hermes legacy (/h/.hermes/scripts/legacy.sh)")
    );
}

#[test]
fn replace_unloads_backs_up_and_removes() {
    let mut sys = fake();
    with_file(
        &mut sys,
        "/h/Library/LaunchAgents/com.crewboard.demo.dev1.plist",
        &plist(
            "com.crewboard.demo.dev1",
            &[
                "/bin/bash",
                "-c",
                &format!("crewboard poll --config-dir {DIR}"),
            ],
        ),
    );
    let t = target("dev1");
    let f = &find_foreign(&host(Os::Mac), &sys, &t)[0];
    let r = replace_plan(&host(Os::Mac), &sys, &t, f, "20261025");
    assert_eq!(
        r.before[0].line(),
        "launchctl bootout gui/501/com.crewboard.demo.dev1"
    );
    assert_eq!(
        r.backups,
        [(
            PathBuf::from("/h/Library/LaunchAgents/com.crewboard.demo.dev1.plist"),
            PathBuf::from("/h/Library/LaunchAgents/com.crewboard.demo.dev1.plist.bak-20261025")
        )]
    );
    assert_eq!(r.remove.len(), 1);
}
