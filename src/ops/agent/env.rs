//! `crewboard agent env`: the export lines of the agent shell, for `eval`.

use super::{Failed, absolute, missing_dir};
use std::path::Path;

/// One word for a POSIX shell: `'a'\''b'`.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The lines for `eval "$(crewboard agent env)"`: the shim dir first on the PATH, the empty `gh`
/// config dir, the config dir. The first two only when the dirs exist. Never a token. Pure.
pub fn env_lines(abs: &Path, has_bin: bool, has_empty_gh_config: bool) -> String {
    let mut out = String::new();
    if has_bin {
        out.push_str(&format!(
            "export PATH={}:\"$PATH\"\n",
            sq(&abs.join("bin").to_string_lossy())
        ));
    }
    if has_empty_gh_config {
        out.push_str(&format!(
            "export GH_CONFIG_DIR={}\n",
            sq(&abs.join("gh-empty-config").to_string_lossy())
        ));
    }
    out.push_str(&format!(
        "export CREWBOARD_CONFIG_DIR={}\n",
        sq(&abs.to_string_lossy())
    ));
    out
}

/// `agent env` for the agent dir `dir`.
pub fn run_env(dir: &Path) -> Result<String, Failed> {
    if !dir.is_dir() {
        return Err(missing_dir(dir).into());
    }
    let abs = absolute(dir);
    Ok(env_lines(
        &abs,
        abs.join("bin").is_dir(),
        abs.join("gh-empty-config").is_dir(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn lines_of_a_full_agent_dir() {
        let t = env_lines(Path::new("/a/dev1"), true, true);
        assert_eq!(
            t,
            "export PATH='/a/dev1/bin':\"$PATH\"\nexport GH_CONFIG_DIR='/a/dev1/gh-empty-config'\nexport CREWBOARD_CONFIG_DIR='/a/dev1'\n"
        );
    }

    #[test]
    fn a_bare_dir_gets_the_config_dir_only() {
        assert_eq!(
            env_lines(Path::new("/a/bare"), false, false),
            "export CREWBOARD_CONFIG_DIR='/a/bare'\n"
        );
    }

    #[test]
    fn a_quote_in_the_path_is_quoted_for_the_shell() {
        let t = env_lines(Path::new("/a/it's"), false, false);
        assert_eq!(t, "export CREWBOARD_CONFIG_DIR='/a/it'\\''s'\n");
    }

    #[test]
    fn a_missing_dir_is_a_config_error() {
        let e = run_env(Path::new("/no/such/agent/dir")).unwrap_err();
        assert!(matches!(e, Failed::Config(_)), "{e:?}");
    }
}
