//! The `crewboard` binary: parse the command line, run it through `crewboard::cli`, and map the
//! result to the exit code (spec/cli#exit-codes). All logic lives in the library.

// The test code of the crate: a test fails by panic and may wait for real processes.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::let_underscore_must_use,
        clippy::disallowed_methods,
    )
)]

use clap::Parser;
use crewboard::cli;
use crewboard::exit::Exit;
use std::io::Write;
use std::process::ExitCode;

/// The Windows main thread has 1 MiB of stack, and the command tree of clap in a debug build
/// needs most of it (a CI run on Windows overflowed it). The work runs on a thread with a larger one.
const STACK_BYTES: usize = 16 * 1024 * 1024;

fn main() -> ExitCode {
    match std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(real_main)
    {
        Ok(h) => h.join().unwrap_or(ExitCode::from(101)),
        Err(_) => real_main(),
    }
}

/// The arguments after the program name. A byte sequence that is not UTF-8 reads as U+FFFD:
/// `std::env::args()` would panic (exit 101), and clap reads the real bytes itself.
fn argv() -> Vec<String> {
    lossy_args(std::env::args_os().skip(1))
}

fn lossy_args(args: impl Iterator<Item = std::ffi::OsString>) -> Vec<String> {
    args.map(|a| a.to_string_lossy().into_owned()).collect()
}

fn real_main() -> ExitCode {
    let args = argv();
    {
        use clap::CommandFactory;
        let topic = if cli::overview::extensions_requested(&args) {
            Some(cli::overview::render_extensions(&cli::Cli::command()))
        } else if cli::overview::scheduler_requested(&args) {
            Some(cli::overview::render_scheduler(&cli::Cli::command()))
        } else {
            None
        };
        if let Some(text) = topic {
            write!(std::io::stdout(), "{text}").ok(); // output is best effort; a closed stream has nobody to tell
            return Exit::Ok.into();
        }
    }
    let cli = match cli::Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            // clap exits with 2 on a usage error, but 2 means "refused by a rule" here. A bad
            // command line is a usage error (64) for every command; help, version and the
            // overview of a bare `crewboard` are 0.
            let code = if e.use_stderr() {
                Exit::Usage
            } else {
                Exit::Ok
            };
            if code == Exit::Usage && is_monitor_form(&args) {
                // a Monitor tool reads standard output: one line, as for every other error
                let json = args.iter().any(|a| a == "--json");
                writeln!(
                    std::io::stdout(),
                    "{}",
                    cli::monitor_usage_line(&e.to_string(), json)
                )
                .ok(); // output is best effort; a closed stream has nobody to tell
                return code.into();
            }
            if let Some(text) = overview_for(&e, &args) {
                // the grouped overview replaces clap's list of every command; a bare `crewboard`
                // asks for it like `--help` does, so it is no usage error
                write!(std::io::stdout(), "{text}").ok(); // output is best effort; a closed stream has nobody to tell
                return Exit::Ok.into();
            }
            e.print().ok(); // output is best effort; a closed stream has nobody to tell
            return code.into();
        }
    };
    let outcome = cli::run(&cli);
    if let Some(m) = &outcome.message {
        eprintln!("{m}");
    }
    ExitCode::from(outcome.exit_code())
}

/// The grouped overview, when the command line asks for the help of `crewboard` itself:
/// no arguments (clap reports a missing subcommand), `-h`, `--help` or `help`. Any other help
/// is clap's own.
fn overview_for(e: &clap::Error, args: &[String]) -> Option<String> {
    use clap::CommandFactory;
    use clap::error::ErrorKind;
    if !matches!(
        e.kind(),
        ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) {
        return None;
    }
    let long = cli::overview::requested(args)?;
    Some(cli::overview::render(&cli::Cli::command(), long))
}

/// Is this command line `monitor`, or `watch --loop` (the same loop)? Both end with one line on
/// standard output, also for a command line that clap refuses.
fn is_monitor_form(args: &[String]) -> bool {
    match first_command(args).as_deref() {
        Some("monitor") => true,
        Some("watch") => args.iter().any(|a| a == "--loop"),
        _ => false,
    }
}

/// The first word of the command line that is not a global option or its value.
fn first_command(args: &[String]) -> Option<String> {
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config-dir" | "--env-file" => {
                args.next();
            }
            _ if a.starts_with("--config-dir=") || a.starts_with("--env-file=") => {}
            _ => return Some(a.clone()),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_read_lossily_instead_of_panicking() {
        let args = lossy_args(
            [
                std::ffi::OsString::from("status"),
                std::ffi::OsString::from("--json"),
            ]
            .into_iter(),
        );
        assert_eq!(args, ["status", "--json"]);
        assert_eq!(first_command(&args).as_deref(), Some("status"));
        assert!(!is_monitor_form(&args));
        let monitor = lossy_args(["watch", "--loop"].map(Into::into).into_iter());
        assert!(is_monitor_form(&monitor));
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_argument_becomes_the_replacement_character() {
        use std::os::unix::ffi::OsStringExt;
        let bad = std::ffi::OsString::from_vec(b"x\xFFy".to_vec());
        assert_eq!(lossy_args([bad].into_iter()), ["x\u{FFFD}y"]);
    }
}
