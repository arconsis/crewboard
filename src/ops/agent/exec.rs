//! `crewboard agent exec`: replace this process by a command that runs with the agent environment.

use super::Failed;
#[cfg(unix)]
use super::{absolute, missing_dir};
use crate::agent_env::TOKEN_VARS;
use std::ffi::{OsStr, OsString};
use std::path::Path;

/// The change of the environment for the child: `(name, Some(value))` sets, `(name, None)`
/// removes. The token variables go; `<dir>/bin` goes first on the PATH when it exists;
/// `GH_CONFIG_DIR` is the empty `gh` config dir when it exists; `CREWBOARD_CONFIG_DIR` is the
/// agent dir. Pure.
pub fn exec_env_plan(
    abs: &Path,
    has_bin: bool,
    has_empty_gh_config: bool,
    old_path: Option<&OsStr>,
) -> Vec<(OsString, Option<OsString>)> {
    let mut plan: Vec<(OsString, Option<OsString>)> = TOKEN_VARS
        .iter()
        .map(|v| (OsString::from(v), None))
        .collect();
    if has_bin {
        let mut dirs = vec![abs.join("bin")];
        if let Some(p) = old_path {
            dirs.extend(std::env::split_paths(p));
        }
        let joined = std::env::join_paths(dirs).unwrap_or_else(|_| abs.join("bin").into());
        plan.push(("PATH".into(), Some(joined)));
    }
    if has_empty_gh_config {
        plan.push((
            "GH_CONFIG_DIR".into(),
            Some(abs.join("gh-empty-config").into_os_string()),
        ));
    }
    plan.push((
        "CREWBOARD_CONFIG_DIR".into(),
        Some(abs.as_os_str().to_os_string()),
    ));
    plan
}

/// `agent exec`: runs `cmd` in place of this process. It returns only when it failed.
#[cfg(unix)]
pub fn run_exec(dir: &Path, cmd: &[String]) -> Failed {
    use std::os::unix::process::CommandExt;
    let Some(program) = cmd.first() else {
        return crate::error::CbError::config("agent exec: no command").into();
    };
    if !dir.is_dir() {
        return missing_dir(dir).into();
    }
    let abs = absolute(dir);
    let mut c = std::process::Command::new(program);
    c.args(&cmd[1..]);
    let old_path = std::env::var_os("PATH");
    for (k, v) in exec_env_plan(
        &abs,
        abs.join("bin").is_dir(),
        abs.join("gh-empty-config").is_dir(),
        old_path.as_deref(),
    ) {
        match v {
            Some(v) => c.env(k, v),
            None => c.env_remove(k),
        };
    }
    super::with_env::start_failed(program, &c.exec())
}

#[cfg(not(unix))]
pub fn run_exec(_dir: &Path, _cmd: &[String]) -> Failed {
    unreachable!("the caller refuses a build without Unix before it gets here")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(plan: &'a [(OsString, Option<OsString>)], k: &str) -> Option<&'a Option<OsString>> {
        plan.iter().find(|(n, _)| n == k).map(|(_, v)| v)
    }

    #[cfg(unix)]
    #[test]
    fn plan_of_a_full_agent_dir() {
        let plan = exec_env_plan(
            Path::new("/a/dev1"),
            true,
            true,
            Some(OsStr::new("/usr/bin:/bin")),
        );
        for v in TOKEN_VARS {
            assert_eq!(get(&plan, v), Some(&None), "{v}");
        }
        assert_eq!(
            get(&plan, "PATH"),
            Some(&Some(OsString::from("/a/dev1/bin:/usr/bin:/bin")))
        );
        assert_eq!(
            get(&plan, "GH_CONFIG_DIR"),
            Some(&Some(OsString::from("/a/dev1/gh-empty-config")))
        );
        assert_eq!(
            get(&plan, "CREWBOARD_CONFIG_DIR"),
            Some(&Some(OsString::from("/a/dev1")))
        );
    }

    #[test]
    fn plan_of_a_bare_dir_leaves_path_and_gh_config_alone() {
        let plan = exec_env_plan(Path::new("/a/bare"), false, false, Some(OsStr::new("/bin")));
        assert_eq!(get(&plan, "PATH"), None);
        assert_eq!(get(&plan, "GH_CONFIG_DIR"), None);
        assert_eq!(
            get(&plan, "CREWBOARD_CONFIG_DIR"),
            Some(&Some(OsString::from("/a/bare")))
        );
        // the token variables go in every case
        assert_eq!(get(&plan, "GH_TOKEN"), Some(&None));
    }

    #[cfg(unix)]
    #[test]
    fn plan_without_a_path_gives_the_shim_dir_alone() {
        let plan = exec_env_plan(Path::new("/a/dev1"), true, false, None);
        assert_eq!(
            get(&plan, "PATH"),
            Some(&Some(OsString::from("/a/dev1/bin")))
        );
    }
}
