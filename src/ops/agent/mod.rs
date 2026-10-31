//! `crewboard agent exec`, `agent env`, `agent with-env`, `agent add` and `agent shim`: run a
//! command as one agent, and set an agent up.
//!
//! An agent dir is the config dir of one agent. `exec` and `env` build the environment that
//! makes a shell an agent shell: the `gh` shim first on the PATH, an empty `gh` config dir, the
//! config dir. No token goes into it (the shim loads the agent token for each `gh` call), and the
//! token variables of the caller go out. `with-env` runs one command with the token of an env
//! file. `add` sets one agent up in seven steps (and the headless tick of a coordinator in
//! three); `shim` writes the `gh` stub. All of them need a Unix host.

#[cfg(unix)]
mod add;
#[cfg(unix)]
mod coordinator;
mod env;
mod exec;
#[cfg(unix)]
mod install_wake;
mod scheduler;
#[cfg(unix)]
mod shim;
#[cfg(unix)]
mod wait;
mod with_env;

#[cfg(unix)]
pub use add::AddArgs;
pub use env::{env_lines, run_env};
pub use exec::{exec_env_plan, run_exec};
#[cfg(unix)]
pub use install_wake::{Installer, Target as WakeTarget, WakeSpec};
pub use scheduler::AddScheduler;
#[cfg(unix)]
pub use shim::{ShimArgs, run_shim};
pub use with_env::run_with_env;

/// Run `agent add`. It prints its own lines. The result is the exit code.
#[cfg(unix)]
pub fn run_add(a: &AddArgs) -> u8 {
    match a.role.unwrap_or(AddRole::Dev) {
        AddRole::HeadlessCoordinator if a.token_stdin => add::refuse_token_stdin_headless(),
        AddRole::HeadlessCoordinator if a.install_wake => add::refuse_install_wake_headless(),
        AddRole::HeadlessCoordinator => coordinator::run_headless(a),
        _ => add::run_add_steps(a),
    }
}

/// The runtime of an agent that `agent add` sets up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AgentRuntime {
    Claude,
    Codex,
    Hermes,
}

impl AgentRuntime {
    /// The name on the command line, and the executable.
    pub fn bin(self) -> &'static str {
        match self {
            AgentRuntime::Claude => "claude",
            AgentRuntime::Codex => "codex",
            AgentRuntime::Hermes => "hermes",
        }
    }

    /// The first word of the runtime label.
    fn label_word(self) -> &'static str {
        match self {
            AgentRuntime::Claude => "claude-code",
            AgentRuntime::Codex => "codex",
            AgentRuntime::Hermes => "hermes",
        }
    }

    /// The runtime label that `join` writes into the agent item, the agent config and the signed
    /// join request: `<word>/<model>`, or `<word>` alone without a model. The word is
    /// `claude-code` for claude, else the runtime name. The one place that builds it.
    pub fn label(self, model: Option<&str>) -> String {
        match model {
            Some(m) => format!("{}/{m}", self.label_word()),
            None => self.label_word().to_string(),
        }
    }

    /// The runtime and the model of a runtime label (the reverse of [`AgentRuntime::label`]).
    /// `None` for a label whose first word is no runtime of `agent add`.
    pub fn from_label(label: &str) -> Option<(AgentRuntime, Option<&str>)> {
        let (word, model) = match label.split_once('/') {
            Some((w, m)) => (w, Some(m).filter(|m| !m.is_empty())),
            None => (label, None),
        };
        let rt = match word.trim() {
            "claude-code" | "claude" => AgentRuntime::Claude,
            "codex" => AgentRuntime::Codex,
            "hermes" | "hermes-agent" => AgentRuntime::Hermes,
            _ => return None,
        };
        Some((rt, model))
    }
}

/// The value parser of `join --runtime`: one of the runtimes of `agent add`. The old form
/// `<runtime>/<model>` (a label) is refused, and the message shows the new form.
pub fn parse_runtime(s: &str) -> Result<AgentRuntime, String> {
    if s.contains('/') {
        return Err(format!(
            "--runtime takes the runtime only (claude, codex or hermes), not <runtime>/<model>; use: {}",
            join_options(s)
        ));
    }
    <AgentRuntime as clap::ValueEnum>::from_str(s, false)
        .map_err(|_| "--runtime must be claude, codex or hermes".to_string())
}

/// The `--runtime` and `--model` options of `join` that write this runtime label. A label whose
/// first word is no runtime of `agent add` gets placeholders.
pub fn join_options(label: &str) -> String {
    match AgentRuntime::from_label(label) {
        Some((rt, Some(m))) => format!("--runtime {} --model {m}", rt.bin()),
        Some((rt, None)) => format!("--runtime {}", rt.bin()),
        None => "--runtime <claude|codex|hermes> --model <model>".to_string(),
    }
}

/// The value parser of `--model`: not empty, no white space and no control character.
pub fn parse_model(s: &str) -> Result<String, String> {
    if s.is_empty() || s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("--model must not be empty and must not have white space".into());
    }
    Ok(s.to_string())
}

/// What `agent add` sets up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AddRole {
    Dev,
    Coordinator,
    HeadlessCoordinator,
}

use crate::error::CbError;
use std::path::{Component, Path, PathBuf};

/// Why a command that should replace this process did not.
#[derive(Debug)]
pub enum Failed {
    /// A configuration problem (exit 4).
    Config(CbError),
    /// The command could not start. `code` is 127 (not found) or 126 (cannot run), as a shell says.
    Start { code: u8, text: String },
}

impl From<CbError> for Failed {
    fn from(e: CbError) -> Self {
        Failed::Config(e)
    }
}

/// The agent dir as an absolute path, without symlink resolution: the way `cd DIR && pwd` shows
/// it. `$PWD` is used for a relative path when it names the current directory.
pub(crate) fn absolute(dir: &Path) -> PathBuf {
    let base = if dir.is_absolute() {
        PathBuf::new()
    } else {
        logical_cwd()
    };
    let mut out = base;
    for c in dir.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn logical_cwd() -> PathBuf {
    let real = std::env::current_dir().unwrap_or_default();
    match std::env::var_os("PWD").map(PathBuf::from) {
        Some(p)
            if p.is_absolute()
                && std::fs::canonicalize(&p).ok() == std::fs::canonicalize(&real).ok() =>
        {
            p
        }
        _ => real,
    }
}

/// The error of a missing agent dir (exit 4).
pub(crate) fn missing_dir(dir: &Path) -> CbError {
    CbError::config(format!(
        "agent dir not found: {} (make it with: crewboard agent add ...)",
        dir.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runtime_label_of_every_runtime_with_and_without_a_model() {
        let all = [
            (AgentRuntime::Claude, "claude", "claude-code"),
            (AgentRuntime::Codex, "codex", "codex"),
            (AgentRuntime::Hermes, "hermes", "hermes"),
        ];
        for (rt, bin, word) in all {
            assert_eq!(rt.bin(), bin);
            assert_eq!(rt.label(Some("m-1")), format!("{word}/m-1"));
            assert_eq!(rt.label(None), word);
            // the reverse gives the runtime and the model back
            assert_eq!(
                AgentRuntime::from_label(&rt.label(Some("m-1"))),
                Some((rt, Some("m-1")))
            );
            assert_eq!(AgentRuntime::from_label(&rt.label(None)), Some((rt, None)));
            assert_eq!(parse_runtime(bin), Ok(rt));
        }
        // a model may have a slash of its own: only the first one ends the runtime word
        assert_eq!(
            AgentRuntime::Hermes.label(Some("vendor/model")),
            "hermes/vendor/model"
        );
        assert_eq!(
            AgentRuntime::from_label("hermes/vendor/model"),
            Some((AgentRuntime::Hermes, Some("vendor/model")))
        );
        assert_eq!(AgentRuntime::from_label("other/x"), None);
    }

    #[test]
    fn join_refuses_the_old_runtime_label_form_and_shows_the_new_one() {
        let e = parse_runtime("claude-code/sonnet").unwrap_err();
        assert!(
            e.contains("not <runtime>/<model>")
                && e.ends_with("use: --runtime claude --model sonnet"),
            "{e}"
        );
        assert!(
            parse_runtime("codex/")
                .unwrap_err()
                .ends_with("use: --runtime codex"),
            "empty model"
        );
        assert!(
            parse_runtime("hermes-agent/m")
                .unwrap_err()
                .ends_with("use: --runtime hermes --model m")
        );
        assert!(
            parse_runtime("fake/it")
                .unwrap_err()
                .ends_with("use: --runtime <claude|codex|hermes> --model <model>")
        );
        // no alias: the label word and an unknown name are no runtime
        for bad in ["claude-code", "copilot", "", "Claude"] {
            assert_eq!(
                parse_runtime(bad),
                Err("--runtime must be claude, codex or hermes".to_string()),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_model_has_no_white_space() {
        assert_eq!(parse_model("sonnet"), Ok("sonnet".to_string()));
        assert_eq!(parse_model("vendor/m-1.5"), Ok("vendor/m-1.5".to_string()));
        for bad in ["", "a b", "a\tb", "a\u{7}"] {
            assert!(parse_model(bad).is_err(), "{bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn absolute_keeps_an_absolute_path_and_folds_dots() {
        assert_eq!(absolute(Path::new("/a/b")), PathBuf::from("/a/b"));
        assert_eq!(absolute(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
    }

    #[test]
    fn absolute_puts_a_relative_path_under_the_current_dir() {
        let a = absolute(Path::new("x/y"));
        assert!(a.is_absolute(), "{a:?}");
        assert!(a.ends_with("x/y"), "{a:?}");
    }
}
