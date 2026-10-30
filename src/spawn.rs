//! Start a child process, and try again when Linux says "Text file busy".
//!
//! Linux refuses to run a file that some process still has open for writing (`ETXTBSY`,
//! raw OS error 26). This happens when a program is replaced while it starts, or when a
//! forked child briefly holds the write handle of a script that was just written. The
//! state clears in a few milliseconds, so a short retry is the right answer. Every other
//! error returns at once. On other systems the helper starts the program once.

use crate::base::ignore::child_may_have_ended;
use std::io;
use std::process::{Child, Command};
use std::time::Duration;

/// The exit code of a finished child: its code, or 128 + the number of the signal that ended
/// it (as a shell reports it), or 1 when neither is known.
pub fn code_of(st: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    if let Some(sig) = std::os::unix::process::ExitStatusExt::signal(&st) {
        return 128 + sig;
    }
    st.code().unwrap_or(1)
}

/// How many times the helper tries to start the program.
const TRIES: u32 = 10;
/// The pause between two tries.
const PAUSE: Duration = Duration::from_millis(20);

/// True when the error is "Text file busy". Only unix knows this error.
#[cfg(unix)]
fn is_text_file_busy(e: &io::Error) -> bool {
    e.raw_os_error() == Some(26)
}

#[cfg(not(unix))]
fn is_text_file_busy(_: &io::Error) -> bool {
    false
}

/// Call `start` until it succeeds, fails with a different error, or used up `tries`.
/// The error of the last try is returned.
fn retry_busy<T>(
    tries: u32,
    pause: Duration,
    mut start: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    let mut attempt = 1;
    loop {
        match start() {
            Err(e) if is_text_file_busy(&e) && attempt < tries => {
                attempt += 1;
                crate::clock::pause(pause);
            }
            other => return other,
        }
    }
}

/// Like `Command::spawn`, but retries when the program is busy (`ETXTBSY`).
pub fn spawn_retrying(cmd: &mut Command) -> io::Result<Child> {
    retry_busy(TRIES, PAUSE, || cmd.spawn())
}

/// Like `Command::output`, but retries when the program is busy (`ETXTBSY`).
pub fn output_retrying(cmd: &mut Command) -> io::Result<std::process::Output> {
    retry_busy(TRIES, PAUSE, || cmd.output())
}

/// Like `Command::status`, but retries when the program is busy (`ETXTBSY`).
pub fn status_retrying(cmd: &mut Command) -> io::Result<std::process::ExitStatus> {
    retry_busy(TRIES, PAUSE, || cmd.status())
}

/// Run `cmd` and collect its output, but kill it when it runs longer than `limit`
/// (`ErrorKind::TimedOut`). The standard input is closed.
#[allow(
    clippy::disallowed_methods,
    reason = "real elapsed time of a child process"
)]
pub fn output_within(
    cmd: &mut std::process::Command,
    limit: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = spawn_retrying(cmd)?;
    let reader = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut v = Vec::new();
            if let Some(mut p) = pipe {
                p.read_to_end(&mut v).ok(); // a read error keeps what was read so far
            }
            v
        })
    };
    let so = reader(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let se = reader(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let end = Instant::now() + limit;
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if Instant::now() >= end {
            // the readers end when the pipes close; do not wait for a grandchild that holds them
            child_may_have_ended(child.kill());
            child_may_have_ended(child.wait());
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("no answer after {} s", limit.as_secs()),
            ));
        }
        crate::clock::pause(Duration::from_millis(20));
    };
    Ok(std::process::Output {
        status,
        stdout: so.join().unwrap_or_default(),
        stderr: se.join().unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn busy() -> io::Error {
        io::Error::from_raw_os_error(26)
    }

    /// Run the helper with a start function that fails `fails` times with `err`.
    #[cfg(unix)]
    fn run(fails: u32, err: fn() -> io::Error) -> (io::Result<&'static str>, u32) {
        let mut calls = 0;
        let r = retry_busy(TRIES, Duration::ZERO, || {
            calls += 1;
            if calls <= fails {
                Err(err())
            } else {
                Ok("started")
            }
        });
        (r, calls)
    }

    #[cfg(unix)]
    #[test]
    fn a_busy_file_is_tried_again_until_it_starts() {
        let (r, calls) = run(2, busy);
        assert_eq!(r.unwrap(), "started");
        assert_eq!(calls, 3);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_that_stays_busy_fails_after_ten_tries() {
        let (r, calls) = run(u32::MAX, busy);
        assert_eq!(r.unwrap_err().raw_os_error(), Some(26));
        assert_eq!(calls, 10);
    }

    /// A script that a writer still holds open is busy on Linux. The helpers wait for the
    /// writer to close it. (On macOS the script starts at once, and the test passes as well.)
    #[cfg(unix)]
    #[test]
    fn a_script_held_open_for_writing_runs_after_the_writer_lets_go() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("stub");
        let mut w = std::fs::File::create(&p).unwrap();
        w.write_all(b"#!/bin/sh\necho ran\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let closer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            drop(w);
        });
        let out = output_retrying(&mut Command::new(&p)).unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "ran\n");
        let st = status_retrying(Command::new(&p).stdout(std::process::Stdio::null())).unwrap();
        assert!(st.success());
        closer.join().unwrap();
    }

    #[test]
    fn any_other_error_returns_at_once() {
        let (r, calls) = {
            let mut calls = 0;
            let r: io::Result<()> = retry_busy(TRIES, Duration::ZERO, || {
                calls += 1;
                Err(io::Error::from(io::ErrorKind::NotFound))
            });
            (r, calls)
        };
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(calls, 1);
    }
}
