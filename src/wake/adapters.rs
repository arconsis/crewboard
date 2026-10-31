//! The wake command of a dev agent for each runtime (Claude Code, Codex, Hermes): the argument
//! vector, the wake prompt and the settings of the events hook. One template for all runtimes;
//! the runtime is a row of a table. No `sh -c` and no quoting happen here: the result is a
//! vector of words that `Command` runs as it is. `--print-cmd` shows it as a shell line.

use super::words;
use std::path::{Path, PathBuf};

/// A runtime that can run a wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Claude,
    Codex,
    Hermes,
}

impl Runtime {
    pub fn parse(s: &str) -> Option<Runtime> {
        match s {
            "claude" => Some(Runtime::Claude),
            "codex" => Some(Runtime::Codex),
            "hermes" => Some(Runtime::Hermes),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Runtime::Claude => "claude",
            Runtime::Codex => "codex",
            Runtime::Hermes => "hermes",
        }
    }

    /// The permission flags of a wake when `--runtime-flags` is not given. Claude's list of
    /// tools is in [`claude_tools`].
    fn default_flags(self, allow: &[String]) -> Vec<String> {
        match self {
            Runtime::Claude => vec![
                "--permission-mode".into(),
                "dontAsk".into(),
                "--allowedTools".into(),
                claude_tools(allow),
            ],
            Runtime::Codex => vec![
                "--sandbox".into(),
                "workspace-write".into(),
                "-c".into(),
                "sandbox_workspace_write.network_access=true".into(),
            ],
            Runtime::Hermes => vec!["-Q".into(), "-s".into(), "crewboard".into()],
        }
    }
}

/// The tools that a Claude wake may use: `crewboard`, `git`, `gh`, the file tools and the shell
/// helpers of compound commands, then each `--allow` entry.
pub fn claude_tools(allow: &[String]) -> String {
    let mut t = String::from(
        "Bash(crewboard *),Bash(git *),Bash(gh *),Read,Edit,Write,Glob,Grep,Bash(echo *),Bash(printf *),Bash(pwd),Bash(test *),Bash(true),Bash(ls *),Bash(cat *),Bash(head *),Bash(tail *),Bash(grep *),Bash(wc *)",
    );
    for a in allow {
        t.push(',');
        t.push_str(a);
    }
    t
}

/// What the argument vector depends on.
pub struct Spec<'a> {
    pub runtime: Runtime,
    /// The runtime binary, a name or a path.
    pub bin: &'a str,
    pub model: Option<&'a str>,
    /// `--runtime-flags`, already split; none: the defaults of the runtime.
    pub flags: Option<Vec<String>>,
    pub allow: &'a [String],
    pub state_dir: &'a Path,
    /// The absolute config dir; Codex may write there.
    pub config_dir: Option<&'a Path>,
    /// The parent dir of each host lock: heavy jobs write their logs there.
    pub host_lock_parents: &'a [PathBuf],
    /// The dirs of the installed skill (see `skill_read_dirs`); a Claude wake may read there.
    pub skill_dirs: &'a [PathBuf],
    /// The `--settings` JSON of a Claude wake (the events hook); none: no hook.
    pub settings_json: Option<&'a str>,
    pub prompt: &'a str,
}

/// The argument vector, the binary first. An error is a wrong combination of options.
pub fn argv(s: &Spec) -> Result<Vec<String>, String> {
    if !s.allow.is_empty() {
        if s.runtime != Runtime::Claude {
            return Err(
                "--allow is for --runtime claude (Codex and Hermes have no tool list)".to_string(),
            );
        }
        if s.flags.is_some() {
            return Err(
                "--allow appends to the default list; with --runtime-flags write the whole list there"
                    .to_string(),
            );
        }
    }
    let flags = s
        .flags
        .clone()
        .unwrap_or_else(|| s.runtime.default_flags(s.allow));
    let mut v = vec![s.bin.to_string()];
    let model = |v: &mut Vec<String>| {
        if let Some(m) = s.model {
            v.push("--model".into());
            v.push(m.to_string());
        }
    };
    let add = |v: &mut Vec<String>, d: &Path| {
        v.push("--add-dir".into());
        v.push(d.to_string_lossy().into_owned());
    };
    match s.runtime {
        Runtime::Claude => {
            v.push("-p".into());
            model(&mut v);
            v.extend(flags);
            // the Read tool needs the state dir for the events file; Claude Code also checks the
            // paths of a Bash command (a redirect to a log) against these dirs
            add(&mut v, s.state_dir);
            for p in s.host_lock_parents {
                add(&mut v, p);
            }
            // the prompt names the skill files: a Bash read (`cat <skill>/references/dev.md`)
            // outside the work dir and the added dirs is refused in dontAsk mode. Claude Code has
            // no read-only variant of --add-dir.
            for p in s.skill_dirs {
                add(&mut v, p);
            }
            if let Some(j) = s.settings_json {
                v.push("--settings".into());
                v.push(j.to_string());
            }
            v.push("--".into());
            v.push(s.prompt.to_string());
        }
        Runtime::Codex => {
            v.push("exec".into());
            model(&mut v);
            v.extend(flags);
            if let Some(c) = s.config_dir {
                add(&mut v, c);
            }
            for p in s.host_lock_parents {
                add(&mut v, p);
            }
            v.push(s.prompt.to_string());
        }
        Runtime::Hermes => {
            v.push("chat".into());
            model(&mut v);
            v.extend(flags);
            v.push("-q".into());
            v.push(s.prompt.to_string());
        }
    }
    Ok(v)
}

/// The dirs of the installed crewboard skill that a headless Claude must read: the shared copy
/// resolved through its symlinks (`--link` points into a clone), and the link dir of Claude that
/// the prompt names. Only dirs that exist; none without a home.
pub fn skill_read_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let Some(home) = home else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    let shared = crate::ops::skill_cmd::canonical_dir(home);
    if let Ok(real) = shared.canonicalize() {
        push(real);
    }
    let link = home.join(".claude/skills/crewboard");
    if let Ok(real) = link.canonicalize() {
        push(link);
        push(real);
    }
    out
}

/// One rule for a refused command; the model must not conclude that Bash is off.
pub const REFUSED_RULE: &str = "If a Bash command is refused, only that command form is refused: run one command per call, without $(...), redirection to files or cd. Write message text with the Write tool to a file in your state dir and pass it with --body-file (crewboard) or -F (git commit). Never stop because of one refused command.";

/// One rule for background work: a runtime that reports a finished background process back into
/// the chat starts a new model turn after the wake has ended, outside its lock, and two writers
/// then share one checkout.
pub const BACKGROUND_RULE: &str = "Never start a background process of your runtime that reports back into this chat when it finishes (for example Hermes notify_on_complete or watch_patterns): run a command in the foreground, or a long one as a detached crewboard job.";

/// The wake prompt, the same for every runtime. The rules are in the skill; the prompt points
/// there and names the host locks of the agent.
pub fn prompt(
    agent: &str,
    initiative: &str,
    events_file: &Path,
    state_dir: &Path,
    host_locks: &[PathBuf],
    config_dir: Option<&Path>,
    extra: Option<&str>,
) -> String {
    let mut p = format!(
        "Use the crewboard skill (SKILL.md and references/dev.md). You are {agent}, a dev agent of {initiative}. Your events are in {}; handle them. Before you act on a ticket, read its thread: crewboard mine <n>. Then work as references/dev.md \"Headless wake: work until done\" says. {REFUSED_RULE} {BACKGROUND_RULE}",
        events_file.display()
    );
    if let Some(c) = config_dir {
        // a runtime may run the model's commands in another process that lacks the wake env
        p.push_str(&format!(
            " Your agent dir is {d}: if a crewboard command says \"no agent config\", add --config-dir {d}; run gh and git push only through the gh shim in {d}/bin (or `crewboard --config-dir {d} agent exec -- <cmd>`).",
            d = c.display()
        ));
    }
    for hl in host_locks {
        p.push_str(&format!(
            " Run every heavy job as crewboard lock run {} --holder {agent} --reason <job> --wait 3600 -- <cmd>. A job longer than the wake: add --detach --result {}/results/<ticket>.result --ticket <ticket>, then end the wake; the next wake names the finished job in your events.",
            hl.display(),
            state_dir.display()
        ));
    }
    if let Some(e) = extra.filter(|e| !e.is_empty()) {
        p.push(' ');
        p.push_str(e);
    }
    p
}

/// What the hook command of a Claude wake needs.
pub struct Hook<'a> {
    /// The absolute path of this binary.
    pub exe: &'a Path,
    pub initiative: &'a str,
    pub every: u32,
    pub log: &'a Path,
    pub config_dir: Option<&'a Path>,
    /// `--crewboard BIN` of the wake (test seam): the hook polls through it too.
    pub crewboard: Option<&'a str>,
}

/// The `--settings` JSON with a `PostToolUse` hook for this wake only. The hook changes no
/// settings file of the user; the timeout is explicit.
pub fn hook_settings(h: &Hook) -> String {
    let mut w = vec![
        h.exe.to_string_lossy().into_owned(),
        "hook".into(),
        "post-tool-use".into(),
        "--initiative".into(),
        h.initiative.into(),
        "--every".into(),
        h.every.to_string(),
        "--log".into(),
        h.log.to_string_lossy().into_owned(),
    ];
    if let Some(c) = h.config_dir {
        w.push("--config-dir".into());
        w.push(c.to_string_lossy().into_owned());
    }
    if let Some(b) = h.crewboard {
        w.push("--crewboard".into());
        w.push(b.into());
    }
    serde_json::json!({
        "hooks": {"PostToolUse": [{
            "matcher": "*",
            "hooks": [{"type": "command", "command": words::join(&w), "timeout": 60}],
        }]}
    })
    .to_string()
}

/// The text of `--print-cmd` and of the log line: `cd <work dir> && <the words>`.
pub fn shell_line(cwd: &Path, argv: &[String]) -> String {
    format!(
        "cd {} && {}",
        words::quote(&cwd.to_string_lossy()),
        words::join(argv)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec<'a>(rt: Runtime, state: &'a Path) -> Spec<'a> {
        Spec {
            runtime: rt,
            bin: "rt",
            model: None,
            flags: None,
            allow: &[],
            state_dir: state,
            config_dir: None,
            host_lock_parents: &[],
            skill_dirs: &[],
            settings_json: None,
            prompt: "P",
        }
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a1_claude_default_argv() {
        let st = Path::new("/s");
        let v = argv(&spec(Runtime::Claude, st)).unwrap();
        assert_eq!(
            v,
            strs(&[
                "rt",
                "-p",
                "--permission-mode",
                "dontAsk",
                "--allowedTools",
                &claude_tools(&[]),
                "--add-dir",
                "/s",
                "--",
                "P"
            ])
        );
        // the shell helpers of compound commands are in the list
        let t = claude_tools(&[]);
        assert!(
            t.starts_with("Bash(crewboard *),Bash(git *),Bash(gh *),Read,Edit,Write,Glob,Grep,")
        );
        assert!(
            t.contains("Bash(echo *)") && t.contains("Bash(test *)") && t.ends_with("Bash(wc *)")
        );
    }

    #[test]
    fn a2_codex_and_a9_hermes_argv_and_a5b_model() {
        let st = Path::new("/s");
        let mut s = spec(Runtime::Codex, st);
        assert_eq!(
            argv(&s).unwrap(),
            strs(&[
                "rt",
                "exec",
                "--sandbox",
                "workspace-write",
                "-c",
                "sandbox_workspace_write.network_access=true",
                "P"
            ])
        );
        s.model = Some("small-1");
        assert_eq!(
            argv(&s).unwrap()[..4],
            strs(&["rt", "exec", "--model", "small-1"])[..]
        );
        let mut h = spec(Runtime::Hermes, st);
        assert_eq!(
            argv(&h).unwrap(),
            strs(&["rt", "chat", "-Q", "-s", "crewboard", "-q", "P"])
        );
        h.model = Some("m");
        assert_eq!(
            argv(&h).unwrap(),
            strs(&[
                "rt",
                "chat",
                "--model",
                "m",
                "-Q",
                "-s",
                "crewboard",
                "-q",
                "P"
            ])
        );
        let mut c = spec(Runtime::Claude, st);
        c.model = Some("m");
        assert_eq!(
            argv(&c).unwrap()[..4],
            strs(&["rt", "-p", "--model", "m"])[..]
        );
        // no model, no --model
        assert!(
            !argv(&spec(Runtime::Claude, st))
                .unwrap()
                .contains(&"--model".to_string())
        );
    }

    #[test]
    fn a5_runtime_flags_replace_the_defaults() {
        let st = Path::new("/s");
        let mut s = spec(Runtime::Claude, st);
        s.flags = Some(words::split("--permission-mode acceptEdits").unwrap());
        let v = argv(&s).unwrap();
        assert!(v.contains(&"acceptEdits".to_string()) && !v.contains(&"dontAsk".to_string()));
        assert_eq!(v[v.len() - 2], "--");
    }

    #[test]
    fn a7_allow_appends_and_a_wrong_combination_is_an_error() {
        let st = Path::new("/s");
        let allow = strs(&["Bash(npm *)", "Bash(npx jest *)"]);
        let mut s = spec(Runtime::Claude, st);
        s.allow = &allow;
        let v = argv(&s).unwrap();
        assert!(
            v.iter()
                .any(|w| w.ends_with(",Bash(wc *),Bash(npm *),Bash(npx jest *)"))
        );
        let mut x = spec(Runtime::Codex, st);
        x.allow = &allow;
        assert!(
            argv(&x)
                .unwrap_err()
                .contains("--allow is for --runtime claude")
        );
        s.flags = Some(vec![]);
        assert!(
            argv(&s)
                .unwrap_err()
                .contains("with --runtime-flags write the whole list there")
        );
    }

    #[test]
    fn a8_add_dirs_for_the_state_dir_the_config_dir_and_the_lock_parents() {
        let st = Path::new("/s");
        let parents = [PathBuf::from("/locks")];
        let mut c = spec(Runtime::Claude, st);
        c.host_lock_parents = &parents;
        c.config_dir = Some(Path::new("/cfg"));
        let adds = |v: &[String]| -> Vec<String> {
            v.windows(2)
                .filter(|w| w[0] == "--add-dir")
                .map(|w| w[1].clone())
                .collect()
        };
        assert_eq!(
            adds(&argv(&c).unwrap()),
            ["/s", "/locks"],
            "claude: state dir, no config dir"
        );
        let mut x = spec(Runtime::Codex, st);
        x.host_lock_parents = &parents;
        x.config_dir = Some(Path::new("/cfg"));
        assert_eq!(
            adds(&argv(&x).unwrap()),
            ["/cfg", "/locks"],
            "codex: config dir, no state dir"
        );
        let h = spec(Runtime::Hermes, st);
        assert!(adds(&argv(&h).unwrap()).is_empty());
    }

    #[test]
    fn a8b_claude_gets_the_skill_dirs_codex_and_hermes_do_not() {
        let st = Path::new("/s");
        let skill = [
            PathBuf::from("/real/skill"),
            PathBuf::from("/h/.claude/skills/crewboard"),
        ];
        let adds = |v: &[String]| -> Vec<String> {
            v.windows(2)
                .filter(|w| w[0] == "--add-dir")
                .map(|w| w[1].clone())
                .collect()
        };
        let mut c = spec(Runtime::Claude, st);
        c.skill_dirs = &skill;
        assert_eq!(
            adds(&argv(&c).unwrap()),
            ["/s", "/real/skill", "/h/.claude/skills/crewboard"]
        );
        // Codex reads the whole disk in its sandbox and writes only in the roots; Hermes has none
        let mut x = spec(Runtime::Codex, st);
        x.skill_dirs = &skill;
        assert!(adds(&argv(&x).unwrap()).is_empty());
        let mut h = spec(Runtime::Hermes, st);
        h.skill_dirs = &skill;
        assert!(adds(&argv(&h).unwrap()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a8c_skill_read_dirs_resolve_the_symlinks_and_skip_what_is_missing() {
        assert!(skill_read_dirs(None).is_empty());
        let t = std::env::temp_dir().join(format!("cb-skilldirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(&t).unwrap();
        assert!(skill_read_dirs(Some(&t)).is_empty(), "no skill installed");
        let clone = t.join("clone/skill");
        std::fs::create_dir_all(&clone).unwrap();
        let shared = t.join(".config/agent-skills/crewboard");
        std::fs::create_dir_all(shared.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&clone, &shared).unwrap();
        let real = clone.canonicalize().unwrap();
        assert_eq!(skill_read_dirs(Some(&t)), vec![real.clone()]);
        let link = t.join(".claude/skills/crewboard");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&shared, &link).unwrap();
        assert_eq!(skill_read_dirs(Some(&t)), [real, link]);
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn a9_a10_the_prompt_is_one_template_and_names_the_host_locks() {
        let p = prompt(
            "dev@host-a",
            "demo",
            Path::new("/s/events.txt"),
            Path::new("/s"),
            &[],
            None,
            None,
        );
        assert_eq!(
            p,
            "Use the crewboard skill (SKILL.md and references/dev.md). You are dev@host-a, a dev agent of demo. Your events are in /s/events.txt; handle them. Before you act on a ticket, read its thread: crewboard mine <n>. Then work as references/dev.md \"Headless wake: work until done\" says. If a Bash command is refused, only that command form is refused: run one command per call, without $(...), redirection to files or cd. Write message text with the Write tool to a file in your state dir and pass it with --body-file (crewboard) or -F (git commit). Never stop because of one refused command. Never start a background process of your runtime that reports back into this chat when it finishes (for example Hermes notify_on_complete or watch_patterns): run a command in the foreground, or a long one as a detached crewboard job."
        );
        assert!(!p.to_lowercase().contains("budget") && !p.contains("poll --quiet"));
        let p = prompt(
            "a",
            "i",
            Path::new("/e"),
            Path::new("/s"),
            &[PathBuf::from("/l/heavy.lock.d")],
            None,
            Some("Don't stop."),
        );
        assert!(p.contains(" Run every heavy job as crewboard lock run /l/heavy.lock.d --holder a --reason <job> --wait 3600 -- <cmd>."));
        assert!(p.contains(
            "--detach --result /s/results/<ticket>.result --ticket <ticket>, then end the wake;"
        ));
        assert!(!p.contains("host-lock.sh"));
        assert!(p.ends_with(" Don't stop."));
    }

    #[test]
    fn the_prompt_names_the_agent_dir_only_when_the_wake_has_one() {
        let with = prompt(
            "a",
            "i",
            Path::new("/e"),
            Path::new("/s"),
            &[],
            Some(Path::new("/cfg/a")),
            None,
        );
        assert!(with.contains(" Your agent dir is /cfg/a: if a crewboard command says \"no agent config\", add --config-dir /cfg/a;"));
        assert!(with.contains("through the gh shim in /cfg/a/bin (or `crewboard --config-dir /cfg/a agent exec -- <cmd>`)."));
        let without = prompt("a", "i", Path::new("/e"), Path::new("/s"), &[], None, None);
        assert!(!without.contains("agent dir") && !without.contains("--config-dir"));
    }

    #[test]
    fn hw1_the_hook_settings_are_json_with_the_command_and_a_timeout() {
        let log = PathBuf::from("/s/wake.log");
        let j = hook_settings(&Hook {
            exe: Path::new("/bin/crewboard"),
            initiative: "demo",
            every: 120,
            log: &log,
            config_dir: Some(Path::new("/cfg dir")),
            crewboard: Some("/t/crewboard"),
        });
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        let h = &v["hooks"]["PostToolUse"][0];
        assert_eq!(h["matcher"], "*");
        assert_eq!(h["hooks"][0]["type"], "command");
        assert_eq!(h["hooks"][0]["timeout"], 60);
        assert_eq!(
            h["hooks"][0]["command"],
            "/bin/crewboard hook post-tool-use --initiative demo --every 120 --log /s/wake.log --config-dir '/cfg dir' --crewboard /t/crewboard"
        );
        // the command reads back as the words it was made from
        let w = words::split(h["hooks"][0]["command"].as_str().unwrap()).unwrap();
        assert_eq!(w[0], "/bin/crewboard");
        assert_eq!(w[w.len() - 3], "/cfg dir");
        // a quote in a path survives
        let j = hook_settings(&Hook {
            exe: Path::new("/it's/crewboard"),
            initiative: "d",
            every: 300,
            log: Path::new("/l"),
            config_dir: None,
            crewboard: None,
        });
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        let w = words::split(
            v["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(w[0], "/it's/crewboard");
    }

    #[test]
    fn the_shell_line_keeps_the_text_shape_of_the_old_wake_command() {
        let v = strs(&[
            "claude",
            "-p",
            "--allowedTools",
            "Bash(git *),Read",
            "--",
            "Use the skill. Don't stop.",
        ]);
        assert_eq!(
            shell_line(Path::new("/w d"), &v),
            "cd '/w d' && claude -p --allowedTools 'Bash(git *),Read' -- 'Use the skill. Don'\\''t stop.'"
        );
    }
}
