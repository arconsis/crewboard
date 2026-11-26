//! Running a step command of `agent add` with a deadline.

use crate::base::ignore::child_may_have_ended;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

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

/// Run a command; stop it after `limit`. Nothing here waits without a deadline: a child that
/// hangs is killed, and a pipe that a grandchild keeps open is given up after a few seconds.
#[allow(
    clippy::disallowed_methods,
    reason = "real elapsed time of a child process"
)]
pub(super) fn run_captured(mut cmd: Command, limit: Duration) -> Captured {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match crate::spawn::spawn_retrying(&mut cmd) {
        Ok(c) => c,
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
    // A reader keeps what it has read in a shared buffer, so that a pipe that never closes (a
    // grandchild holds it) still gives the text up to that point.
    type Shared = (
        std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
        mpsc::Receiver<()>,
    );
    let pipe = |r: Option<Box<dyn Read + Send>>| -> Shared {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel();
        let shared = buf.clone();
        std::thread::spawn(move || {
            if let Some(mut r) = r {
                let mut chunk = [0u8; 4096];
                while let Ok(n) = r.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut b) = shared.lock() {
                        b.extend_from_slice(&chunk[..n]);
                    }
                }
            }
            tx.send(()).ok(); // the receiver may be gone
        });
        (buf, rx)
    };
    let out = pipe(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let err = pipe(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let start = Instant::now();
    let (code, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(st)) => break (st.code().unwrap_or(-1), false),
            Ok(None) if start.elapsed() >= limit => {
                child_may_have_ended(child.kill());
                child_may_have_ended(child.wait());
                break (-1, true);
            }
            Ok(None) => crate::clock::pause(Duration::from_millis(10)),
            Err(_) => {
                child_may_have_ended(child.kill());
                child_may_have_ended(child.wait());
                break (-1, false);
            }
        }
    };
    let grab = |(buf, done): Shared| {
        done.recv_timeout(Duration::from_secs(3)).ok(); // a timeout means the reader is still busy: take what it has
        buf.lock().map(|b| b.clone()).unwrap_or_default()
    };
    let mut text = String::from_utf8_lossy(&grab(out)).to_string();
    text.push_str(&String::from_utf8_lossy(&grab(err)));
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
