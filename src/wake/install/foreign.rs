//! Wake jobs of the same agent under other names. A host may run a job that an older setup or
//! a person made (`com.example.<initiative>.<agent>` with a `poll.sh`, a wake.sh that calls
//! `agent-wake.sh --config-dir DIR`). `wake install` must not start a second loop for the
//! agent: it finds these jobs, refuses, and with `--replace` removes them (each file is copied
//! to `<file>.bak-<date>` first).
//!
//! A job runs a wake for the agent when its command, or the script that its command names
//! (one level), mentions the config dir of the agent and the word `wake` or `poll`.

use super::Scheduler;
use super::plan::{Cmd, Host, OutFile, Removal, Target, crontab, names, wrapper_path};
use super::sys::Sys;
use super::units::cron_marker;
use std::path::{Path, PathBuf};

/// A job that is not ours and wakes the same agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Foreign {
    pub scheduler: Scheduler,
    pub name: String,
    /// The unit file or script; `None` for a crontab line.
    pub file: Option<PathBuf>,
    /// The launchd label (the file stem when the plist has none); the crontab line for cron.
    pub label: String,
}

impl Foreign {
    /// `another wake job runs this agent: <scheduler> <name> (<file>)`.
    pub fn line(&self) -> String {
        let at = self
            .file
            .as_ref()
            .map_or("crontab".to_string(), |f| f.display().to_string());
        format!(
            "another wake job runs this agent: {} {} ({at})",
            self.scheduler.label(),
            self.name
        )
    }
}

fn path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

/// Does `text` name `dir` as a whole path (not as a prefix of `dir-headless`, not as the tail of
/// a longer path)? `~`, `$HOME` and `${HOME}` spellings of a dir below the home dir count too.
fn mentions_dir(text: &str, dir: &Path, home: &Path) -> bool {
    let full = dir.to_string_lossy().into_owned();
    let mut spellings = vec![full];
    if let Ok(rest) = dir.strip_prefix(home) {
        let rest = rest.to_string_lossy();
        for h in ["~", "$HOME", "${HOME}"] {
            spellings.push(format!("{h}/{rest}"));
        }
    }
    spellings.iter().any(|sp| {
        let mut from = 0;
        while let Some(i) = text[from..].find(sp.as_str()) {
            let at = from + i;
            let before = text[..at].chars().next_back();
            let after = text[at + sp.len()..].chars().next();
            let tail_ok = after.is_none_or(|c| !path_char(c));
            // a leading `/` (or `~`, `$`) must not continue a longer path
            let head_ok = before.is_none_or(|c| !(path_char(c) || c == '/'));
            if head_ok && tail_ok {
                return true;
            }
            from = at + sp.len();
        }
        false
    })
}

/// Words of a command line, without quotes.
fn words(cmd: &str) -> Vec<String> {
    cmd.split_whitespace()
        .map(|w| w.trim_matches(|c| c == '"' || c == '\'').to_string())
        .collect()
}

/// The command text and the text of the scripts that it names, one level deep.
fn job_text(sys: &dyn Sys, words: &[String]) -> String {
    let mut t = words.join(" ");
    for w in words {
        let p = Path::new(w);
        if p.is_absolute()
            && let Some(body) = sys.read_text(p)
        {
            t.push('\n');
            t.push_str(&body);
        }
    }
    t
}

fn runs_wake(sys: &dyn Sys, host: &Host, t: &Target, words: &[String], extra: &str) -> bool {
    let text = format!("{}\n{extra}", job_text(sys, words));
    (text.contains("wake") || text.contains("poll"))
        && mentions_dir(&text, &t.config_dir, &host.home)
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The `<string>` values between `start` and the next `</array>`.
fn plist_strings(text: &str, key: &str) -> Vec<String> {
    let Some(i) = text.find(&format!("<key>{key}</key>")) else {
        return Vec::new();
    };
    let rest = &text[i..];
    let end = rest.find("</array>").unwrap_or(rest.len());
    let mut out = Vec::new();
    let mut s = &rest[..end];
    while let Some(a) = s.find("<string>") {
        let Some(b) = s[a..].find("</string>") else {
            break;
        };
        out.push(unescape(&s[a + 8..a + b]));
        s = &s[a + b + 9..];
    }
    out
}

fn plist_label(text: &str) -> Option<String> {
    let i = text.find("<key>Label</key>")?;
    let rest = &text[i..];
    let a = rest.find("<string>")?;
    let b = rest.find("</string>")?;
    (a < b).then(|| unescape(&rest[a + 8..b]))
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn has_ext(p: &Path, ext: &str) -> bool {
    p.extension().is_some_and(|e| e == ext)
}

/// Every wake job of another name that runs this agent.
pub fn find_foreign(host: &Host, sys: &dyn Sys, t: &Target) -> Vec<Foreign> {
    let n = names(&t.initiative, &t.config_dir);
    let own = [n.current.clone(), n.old.clone()];
    let mut out = Vec::new();
    // launchd
    for f in sys.list(&host.home.join("Library/LaunchAgents")) {
        let Some(text) = sys.read_text(&f).filter(|_| has_ext(&f, "plist")) else {
            continue;
        };
        let label = plist_label(&text).unwrap_or_else(|| stem(&f));
        if own.contains(&stem(&f)) || own.contains(&label) {
            continue;
        }
        if runs_wake(sys, host, t, &plist_strings(&text, "ProgramArguments"), "") {
            out.push(Foreign {
                scheduler: Scheduler::Launchd,
                name: label.clone(),
                file: Some(f),
                label,
            });
        }
    }
    // systemd user services
    for f in sys.list(&host.systemd_dir()) {
        let Some(text) = sys.read_text(&f).filter(|_| has_ext(&f, "service")) else {
            continue;
        };
        if own.contains(&stem(&f)) {
            continue;
        }
        let w: Vec<String> = text
            .lines()
            .filter_map(|l| l.trim().strip_prefix("ExecStart="))
            .flat_map(|c| words(c.trim_start_matches(['-', '@', ':', '+', '!'])))
            .collect();
        if runs_wake(sys, host, t, &w, "") {
            out.push(Foreign {
                scheduler: Scheduler::Systemd,
                name: stem(&f),
                label: stem(&f),
                file: Some(f),
            });
        }
    }
    // crontab
    for line in crontab(sys).lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') || own.iter().any(|o| l.contains(&cron_marker(o))) {
            continue;
        }
        if runs_wake(sys, host, t, &words(l), "") {
            let name = l.split("# crewboard:").nth(1).map_or_else(
                || l.chars().take(60).collect::<String>(),
                |m| m.trim().to_string(),
            );
            out.push(Foreign {
                scheduler: Scheduler::Cron,
                name,
                file: None,
                label: line.to_string(),
            });
        }
    }
    // Hermes: the script is the job
    let own_scripts: Vec<PathBuf> = own
        .iter()
        .map(|o| wrapper_path(host, t, Scheduler::Hermes, o))
        .collect();
    for f in sys.list(&host.home.join(".hermes/scripts")) {
        if own_scripts.contains(&f) || !has_ext(&f, "sh") {
            continue;
        }
        if let Some(text) = sys.read_text(&f)
            && runs_wake(sys, host, t, &[], &text)
        {
            out.push(Foreign {
                scheduler: Scheduler::Hermes,
                name: stem(&f),
                label: stem(&f),
                file: Some(f),
            });
        }
    }
    out
}

fn backup_of(f: &Path, date: &str) -> PathBuf {
    let mut s = f.as_os_str().to_os_string();
    s.push(format!(".bak-{date}"));
    PathBuf::from(s)
}

/// What `--replace` does for one foreign job: unload it, copy its file to `<file>.bak-<date>`,
/// remove it.
pub fn replace_plan(host: &Host, sys: &dyn Sys, t: &Target, f: &Foreign, date: &str) -> Removal {
    let mut r = Removal {
        scheduler: f.scheduler,
        name: f.name.clone(),
        before: Vec::new(),
        remove: Vec::new(),
        after: Vec::new(),
        hint: None,
        backups: Vec::new(),
        saves: Vec::new(),
    };
    if let Some(file) = &f.file {
        r.backups.push((file.clone(), backup_of(file, date)));
        r.remove.push(file.clone());
    }
    match f.scheduler {
        Scheduler::Launchd => r.before.push(
            Cmd::new(
                "launchctl",
                &["bootout", &format!("gui/{}/{}", host.uid, f.label)],
            )
            .tolerant(),
        ),
        Scheduler::Systemd => {
            for unit in [format!("{}.timer", f.name), format!("{}.service", f.name)] {
                r.before
                    .push(Cmd::new("systemctl", &["--user", "disable", "--now", &unit]).tolerant());
            }
            r.after
                .push(Cmd::new("systemctl", &["--user", "daemon-reload"]).tolerant());
        }
        Scheduler::Cron => {
            let all = crontab(sys);
            let text: String = all
                .lines()
                .filter(|l| *l != f.label)
                .map(|l| format!("{l}\n"))
                .collect();
            r.saves.push(OutFile {
                path: t.config_dir.join(format!("crontab.bak-{date}")),
                content: all,
                mode: 0o600,
            });
            let mut c = Cmd::new("crontab", &["-"]);
            c.stdin = Some(text);
            r.before.push(c);
        }
        Scheduler::Hermes => {
            r.hint = Some(format!(
                "remove the Hermes job of {} too: hermes cron list; hermes cron remove <job-id>",
                f.name
            ));
        }
        Scheduler::Auto => {}
    }
    r
}
