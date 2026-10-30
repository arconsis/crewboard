//! `agent add --install-wake`: after a good `start`, install the scheduler unit of the dev agent
//! with the code of `crewboard wake install`, in this process. The options are the defaults of
//! `wake install` (scheduler `auto`, every 5 minutes) unless the flags say otherwise. This file
//! knows nothing of `add` and nothing of `wake`: `add` passes the facts of the agent in a
//! `Target`, and the command layer passes the `Installer` that calls `wake install`.

use super::scheduler::AddScheduler;
use crate::clock;
use crate::error::CbError;
use std::path::Path;

/// What `--install-wake` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeSpec {
    pub scheduler: AddScheduler,
    /// The interval of `wake install`, for example `5m`.
    pub every: String,
    /// Extra tools for the wake, as `wake install --allow` takes them (claude only).
    pub allow: Vec<String>,
}

/// The default interval of `wake install`.
pub const DEFAULT_EVERY: &str = "5m";

impl WakeSpec {
    /// The spec of the flags, checked before any write.
    pub fn new(
        scheduler: Option<AddScheduler>,
        every: Option<&str>,
        allow: &[String],
    ) -> Result<WakeSpec, String> {
        let every = every.unwrap_or(DEFAULT_EVERY);
        if clock::parse_secs(every).filter(|s| *s > 0).is_none() {
            return Err(format!(
                "--every needs a duration such as 5m, got '{every}'"
            ));
        }
        Ok(WakeSpec {
            scheduler: scheduler.unwrap_or_default(),
            every: every.to_string(),
            allow: allow.to_vec(),
        })
    }

    /// The flags of the spec, for the command that a rerun prints. Only what is not default.
    pub fn flags(&self) -> String {
        let mut s = String::from(" --install-wake");
        if self.scheduler != AddScheduler::Auto {
            s.push_str(&format!(" --scheduler {}", self.scheduler.label()));
        }
        if self.every != DEFAULT_EVERY {
            s.push_str(&format!(" --every {}", self.every));
        }
        for a in &self.allow {
            s.push_str(&format!(" --allow {}", super::shim::sq(a)));
        }
        s
    }
}

/// The agent that gets the unit.
pub struct Target<'a> {
    pub agent_dir: &'a Path,
    pub initiative: &'a str,
    pub name: &'a str,
    pub runtime: &'a str,
    pub work_dir: &'a Path,
    pub env_file: &'a Path,
}

/// What `wake install` printed: the status line, and the one command left, if there is one.
#[derive(Debug, PartialEq, Eq)]
pub struct Installed {
    pub status: Vec<String>,
    pub next: Option<String>,
}

/// What installs the unit: `wake install` in this process. The command layer passes it in
/// (`ops` does not import `wake`); it returns the text of `wake install`. A refusal (a job of
/// another name wakes this agent) or a failure is its error, with the fix line.
pub type Installer = fn(&WakeSpec, &Target<'_>) -> Result<String, CbError>;

/// Install the unit with `installer`, and split its text.
pub fn install(
    installer: Installer,
    spec: &WakeSpec,
    t: &Target<'_>,
) -> Result<Installed, CbError> {
    installer(spec, t).map(|text| parse(&text))
}

/// Split the text of `wake install` into the status lines and the `next:` command.
fn parse(text: &str) -> Installed {
    let mut status = Vec::new();
    let mut next = None;
    for l in text.lines().filter(|l| !l.trim().is_empty()) {
        match l.strip_prefix("next: ") {
            Some(cmd) => next = Some(cmd.to_string()),
            None => status.push(l.to_string()),
        }
    }
    Installed { status, next }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::ids;

    #[test]
    fn the_flags_check_the_interval_and_a_rerun_line_keeps_only_what_is_not_default() {
        let d = WakeSpec::new(None, None, &[]).unwrap();
        assert_eq!(d.flags(), " --install-wake");
        let s = WakeSpec::new(
            Some(AddScheduler::Cron),
            Some("10m"),
            &["Bash(make:*)".to_string()],
        )
        .unwrap();
        assert_eq!(
            s.flags(),
            " --install-wake --scheduler cron --every 10m --allow 'Bash(make:*)'"
        );
        for bad in ["", "0", "0m", "soon"] {
            let e = WakeSpec::new(None, Some(bad), &[]).unwrap_err();
            assert!(e.contains("--every needs a duration"), "{bad}: {e}");
        }
    }

    #[test]
    fn the_installer_text_is_split_and_an_error_comes_back_whole() {
        let spec = WakeSpec::new(None, None, &[]).unwrap();
        let t = Target {
            agent_dir: Path::new("/a"),
            initiative: "demo",
            name: "x@h",
            runtime: "claude",
            work_dir: Path::new("/w"),
            env_file: Path::new("/e"),
        };
        let ok: Installer = |_, _| Ok("installed u (cron, every 5m); log: /l\n".into());
        assert_eq!(install(ok, &spec, &t).unwrap().status.len(), 1);
        let bad: Installer = |_, _| Err(CbError::refused(ids::SECOND_WAKE_JOB, "x\nfix: y"));
        assert_eq!(install(bad, &spec, &t).unwrap_err().exit().code(), 2);
    }

    #[test]
    fn the_text_of_wake_install_splits_into_status_and_next() {
        let i = parse(
            "installed u (systemd, every 5m); log: /l/wake.log\nnext: loginctl enable-linger \"$USER\"\n",
        );
        assert_eq!(
            i.status,
            ["installed u (systemd, every 5m); log: /l/wake.log"]
        );
        assert_eq!(i.next.as_deref(), Some("loginctl enable-linger \"$USER\""));
        let j = parse("installed u (launchd, every 5m); log: /l\n");
        assert_eq!(j.next, None);
    }
}
