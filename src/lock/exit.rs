//! The exit codes of `lock` (spec/host-lock; the table is docs/reference/exit-codes.md). `run`
//! returns the code of its command in place of [`LockExit::Ok`], so the functions of this module
//! return an `i32` and name each of their own codes here.

/// One exit code of `lock`. Not every subcommand returns every code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockExit {
    /// Got the lock, released it, or the answer is yes (`status`: held; `ci-status`: CI present).
    Ok = 0,
    /// `take`, `run`: the lock directory or its owner file cannot be made. `status`: the lock is
    /// free. `ci-status`: no CI job. `ci-wait`: the waiting list cannot be written.
    No = 1,
    /// `status`: the lock is stale.
    Stale = 2,
    /// `release`, `break --if-dead`: another (live) owner holds the lock; it stays.
    NotOwner = 3,
    /// A bad command line.
    Usage = 64,
    /// A live owner holds the lock, after `--wait`.
    Held = 75,
    /// A CI job runs or waits, after `--wait` (agents only).
    CiActive = 76,
    /// Your own ancestor process holds the lock and gave no grant (no wait).
    HeldByAncestor = 77,
    /// TERM, INT or HUP stopped the command (128 + 15).
    Stopped = 143,
}

impl LockExit {
    pub const fn code(self) -> i32 {
        self as i32
    }
}

#[cfg(test)]
mod tests {
    use super::LockExit;

    #[test]
    fn the_codes_are_the_codes_of_the_exit_table() {
        let codes = [
            LockExit::Ok,
            LockExit::No,
            LockExit::Stale,
            LockExit::NotOwner,
            LockExit::Usage,
            LockExit::Held,
            LockExit::CiActive,
            LockExit::HeldByAncestor,
            LockExit::Stopped,
        ]
        .map(LockExit::code);
        assert_eq!(codes, [0, 1, 2, 3, 64, 75, 76, 77, 143]);
    }
}
