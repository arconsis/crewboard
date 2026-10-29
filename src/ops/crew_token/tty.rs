//! Read one line from the terminal without echo. Ctrl-C puts the terminal back first.

use std::io;

#[cfg(unix)]
pub(super) fn read_hidden(prompt: &str) -> io::Result<String> {
    use std::sync::OnceLock;

    /// The terminal settings before the prompt, for the Ctrl-C handler.
    static SAVED: OnceLock<libc::termios> = OnceLock::new();

    extern "C" fn on_interrupt(sig: libc::c_int) {
        // only async-signal-safe calls: restore the terminal, end the line, die of the signal
        if let Some(t) = SAVED.get() {
            // SAFETY: tcsetattr, write, signal and raise are async-signal-safe; `t` is a valid termios.
            unsafe { libc::tcsetattr(0, libc::TCSANOW, t) };
        }
        // SAFETY: as above; the buffer is one static byte.
        unsafe {
            libc::write(2, b"\n".as_ptr().cast(), 1);
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    }

    // SAFETY: an all-zero termios is a valid buffer for tcgetattr to fill.
    let mut old: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: fd 0 and a valid pointer.
    if unsafe { libc::tcgetattr(0, &mut old) } != 0 {
        return Err(io::Error::last_os_error());
    }
    SAVED.get_or_init(|| old);
    let mut quiet = old;
    quiet.c_lflag &= !libc::ECHO;
    quiet.c_lflag |= libc::ECHONL;
    eprint!("{prompt}");
    // SAFETY: the handler is an `extern "C"` function that lives for the whole process.
    let previous = unsafe {
        libc::signal(
            libc::SIGINT,
            on_interrupt as *const () as libc::sighandler_t,
        )
    };
    // SAFETY: fd 0 and a valid pointer.
    let set = unsafe { libc::tcsetattr(0, libc::TCSAFLUSH, &quiet) };
    let read = if set == 0 {
        let mut line = String::new();
        io::stdin().read_line(&mut line).map(|_| line)
    } else {
        Err(io::Error::last_os_error())
    };
    // SAFETY: fd 0, the settings that tcgetattr gave, and the handler that was there before.
    unsafe {
        libc::tcsetattr(0, libc::TCSAFLUSH, &old);
        libc::signal(libc::SIGINT, previous);
    }
    read
}

#[cfg(not(unix))]
pub(super) fn read_hidden(prompt: &str) -> io::Result<String> {
    eprint!("{prompt}");
    let mut line = String::new();
    io::stdin().read_line(&mut line).map(|_| line)
}
