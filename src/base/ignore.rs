//! Results that a caller drops on purpose. Each helper names the reason, so the call site says
//! why the error does not matter and a search finds every place of one kind. A drop with another
//! reason stays a `.ok()` with its own comment.

/// Drop the result of a write to stdout, stderr or a log: output is best effort, and when the
/// stream is closed, nobody is left to tell.
pub fn best_effort_output<T, E>(result: Result<T, E>) {
    result.ok();
}

/// Drop the result of a removal of a temp file, a stale file or a dir: a leftover is harmless.
pub fn best_effort_cleanup<T, E>(result: Result<T, E>) {
    result.ok();
}

/// Drop the result of `kill`, `wait` or `try_wait` of a child process: the child may have ended
/// (and been reaped) already.
pub fn child_may_have_ended<T, E>(result: Result<T, E>) {
    result.ok();
}
