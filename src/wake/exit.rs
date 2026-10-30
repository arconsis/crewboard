//! The exit codes of `wake` (the table is docs/reference/exit-codes.md). A board call passes its
//! own code through (2, 3, 4); a signal gives 128 plus its number.

/// One exit code of `wake` that the wake itself decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeExit {
    /// Nothing to do, woke OK, a wake already runs, the budget ended it, or the wakes are
    /// stopped.
    Ok = 0,
    /// GitHub or the network did not deliver (from a board call).
    Network = 3,
    /// A configuration or key error: the stop file is written.
    Config = 4,
    /// A bad command line.
    Usage = 64,
    /// The wake command failed.
    WakeFailed = 70,
    /// The wake lock cannot be taken.
    LockUnavailable = 71,
}

impl WakeExit {
    pub const fn code(self) -> u8 {
        self as u8
    }
}

#[cfg(test)]
mod tests {
    use super::WakeExit;

    #[test]
    fn the_codes_are_the_codes_of_the_exit_table() {
        let codes = [
            WakeExit::Ok,
            WakeExit::Network,
            WakeExit::Config,
            WakeExit::Usage,
            WakeExit::WakeFailed,
            WakeExit::LockUnavailable,
        ]
        .map(WakeExit::code);
        assert_eq!(codes, [0, 3, 4, 64, 70, 71]);
    }
}
