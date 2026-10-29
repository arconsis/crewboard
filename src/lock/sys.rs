//! The few system calls of the lock that `std` has no call for: signals to a child and to a process
//! group, a handler that records TERM, INT and HUP, a new session for a detached job. No crate
//! besides `libc`, which is a dependency already.

use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicI32, Ordering};

static CAUGHT: AtomicI32 = AtomicI32::new(0);
/// The last signal that was recorded; [`caught`] does not clear it. It lives as long as the
/// process and is never reset, so it is for single-shot callers only (the gate, `lock run`): a
/// process that goes on after a signal would see it for ever.
static LAST: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_signal(sig: libc::c_int) {
    // async-signal-safe: atomic stores
    CAUGHT.store(sig, Ordering::SeqCst);
    LAST.store(sig, Ordering::SeqCst);
}

/// From now on TERM, INT and HUP are recorded, not fatal. [`caught`] reads them.
pub fn catch_term_int_hup() {
    install(0);
}

/// [`catch_term_int_hup`] with `SA_RESTART`: a system call that a signal interrupts goes on, so a
/// caller that makes network calls does not see the signal as an error.
pub fn catch_term_int_hup_restart() {
    install(libc::SA_RESTART);
}

fn install(flags: libc::c_int) {
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: a zeroed sigaction with a handler that only stores an atomic is valid.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as *const () as libc::sighandler_t;
            libc::sigemptyset(&mut sa.sa_mask);
            sa.sa_flags = flags;
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}

/// The last signal that [`catch_term_int_hup`] recorded; the record is cleared.
pub fn caught() -> Option<i32> {
    match CAUGHT.swap(0, Ordering::SeqCst) {
        0 => None,
        s => Some(s),
    }
}

/// The first signal of TERM, INT and HUP that arrived since [`catch_term_int_hup`], also after
/// [`caught`] has read it. A caller that wants to stop at the next safe point asks this.
pub fn stop_requested() -> Option<i32> {
    match LAST.load(Ordering::SeqCst) {
        0 => None,
        s => Some(s),
    }
}

/// Ignore HUP (a detached job must not die with the terminal of its caller).
pub fn ignore_hup() {
    // SAFETY: SIG_IGN is a valid disposition.
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
}

/// Send `sig` to one process.
pub fn signal_pid(pid: u32, sig: i32) {
    if let Ok(p) = i32::try_from(pid) {
        // SAFETY: kill has no memory effect.
        unsafe {
            libc::kill(p, sig);
        }
    }
}

/// Send `sig` to the whole process group whose leader is `pgid`.
pub fn signal_group(pgid: u32, sig: i32) {
    if let Ok(p) = i32::try_from(pgid) {
        // SAFETY: kill has no memory effect.
        unsafe {
            libc::kill(-p, sig);
        }
    }
}

/// Does any process of the group whose leader is `pgid` exist (also one of another user)?
pub fn group_alive(pgid: u32) -> bool {
    let Ok(p) = i32::try_from(pgid) else {
        return false;
    };
    // SAFETY: signal 0 only checks that the group exists.
    let rc = unsafe { libc::kill(-p, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Start the command in a new session (`setsid`), so that a kill of the process group of the
/// caller cannot reach it.
pub fn new_session(cmd: &mut Command) {
    // SAFETY: setsid is async-signal-safe; the closure does nothing else.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}

/// The short host name (`hostname -s`).
pub fn short_hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its length.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    let full = String::from_utf8_lossy(&buf[..end]).to_string();
    let short = full.split('.').next().unwrap_or("").to_string();
    (!short.is_empty()).then_some(short)
}
