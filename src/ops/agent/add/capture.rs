//! Running a step command of `agent add` with a deadline.

use crate::spawn::{Drain, Ended, run_within};
use std::process::Command;
use std::time::Duration;

/// How long one step command may run. A clone is the slow one.
pub(super) const STEP_LIMIT: Duration = Duration::from_secs(180);
pub(super) const CLONE_LIMIT: Duration = Duration::from_secs(900);

/// What a command printed (standard output, then standard error) and how it ended.
pub(super) struct Captured {
    pub text: String,
    /// The exit code; -1 when a signal ended it or it ran too long.
    pub code: i32,
    pub timed_out: bool,
}

/// How long the output of a finished or killed command is waited for, per pipe.
const PIPE_GRACE: Duration = Duration::from_secs(3);

/// Run a command; stop it after `limit`. Nothing here waits without a deadline: a child that
/// hangs is killed, and a pipe that a grandchild keeps open is given up after a few seconds.
pub(super) fn run_captured(mut cmd: Command, limit: Duration) -> Captured {
    let run = match run_within(&mut cmd, limit, Drain::AtMost(PIPE_GRACE)) {
        Ok(r) => r,
        Err(e) => {
            let code = if e.kind() == std::io::ErrorKind::NotFound {
                127
            } else {
                126
            };
            return Captured {
                text: format!("cannot start {}: {e}", cmd.get_program().to_string_lossy()),
                code,
                timed_out: false,
            };
        }
    };
    let (code, timed_out) = match run.ended {
        Ended::Exited(st) => (st.code().unwrap_or(-1), false),
        Ended::TimedOut => (-1, true),
        Ended::WaitFailed(_) => (-1, false),
    };
    let mut text = String::from_utf8_lossy(&run.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&run.stderr));
    if timed_out {
        text.push_str(&format!(
            "\ntimed out after {} s and stopped",
            limit.as_secs()
        ));
    }
    Captured {
        text,
        code,
        timed_out,
    }
}

/// A process exit code as the code of this command: a signal or a timeout is a failure (1).
pub(super) fn exit_of(code: i32) -> u8 {
    u8::try_from(code).ok().filter(|c| *c != 0).unwrap_or(1)
}
