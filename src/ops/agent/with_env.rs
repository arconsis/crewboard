//! `crewboard agent with-env --env-file F -- CMD...`: run one command with the variables of an
//! env file, and print nothing of them.

use super::Failed;

/// What a shell says for a command that did not start: 127 when it is not found, else 126.
#[cfg(unix)]
pub(super) fn start_failed(program: &str, e: &std::io::Error) -> Failed {
    if e.kind() == std::io::ErrorKind::NotFound {
        Failed::Start {
            code: 127,
            text: format!("{program}: command not found"),
        }
    } else {
        Failed::Start {
            code: 126,
            text: format!("{program}: {e}"),
        }
    }
}

/// `agent with-env`: the env file is loaded already (`cli::run` does it before the command), so
/// the child gets its keys; the token variables of the caller are replaced. It returns only when
/// the command did not start.
#[cfg(unix)]
pub fn run_with_env(cmd: &[String]) -> Failed {
    use std::os::unix::process::CommandExt;
    let Some(program) = cmd.first() else {
        return crate::error::CbError::config("agent with-env: no command").into();
    };
    let Some(env) = crate::agent_env::current() else {
        return crate::error::CbError::usage("agent with-env: --env-file is required").into();
    };
    let mut c = std::process::Command::new(program);
    c.args(&cmd[1..]);
    env.apply(&mut c);
    start_failed(program, &c.exec())
}

#[cfg(not(unix))]
pub fn run_with_env(_cmd: &[String]) -> Failed {
    unreachable!("the caller refuses a build without Unix before it gets here")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_missing_program_is_127_and_another_failure_126() {
        let nf = std::io::Error::from(std::io::ErrorKind::NotFound);
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(matches!(
            start_failed("x", &nf),
            Failed::Start { code: 127, .. }
        ));
        assert!(matches!(
            start_failed("x", &denied),
            Failed::Start { code: 126, .. }
        ));
    }
}
