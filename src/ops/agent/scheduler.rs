//! The values of `--scheduler` of `agent add`. The enum carries no Unix-only payload: the
//! command line parses it on every host, so it must exist on every host.

/// The values of `--scheduler`: the ones that `wake install` takes for a dev agent on a
/// host with its own scheduler (Hermes runs from its cron job: `auto` picks that).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AddScheduler {
    #[default]
    Auto,
    Launchd,
    Systemd,
    Cron,
}

impl AddScheduler {
    /// The name on the command line.
    pub fn label(self) -> &'static str {
        match self {
            AddScheduler::Auto => "auto",
            AddScheduler::Launchd => "launchd",
            AddScheduler::Systemd => "systemd",
            AddScheduler::Cron => "cron",
        }
    }
}
