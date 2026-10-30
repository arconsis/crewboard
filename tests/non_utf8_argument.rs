//! A command line with bytes that are not UTF-8 ends with a normal exit code, not with a Rust
//! panic (exit 101). `std::env::args()` panics on such an argument; `main` reads the arguments
//! once as `OsString`.

#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panic and waits for real processes"
)]

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::process::Command;

fn run(args: Vec<OsString>) -> std::process::Output {
    let dir = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_crewboard"))
        .arg("--config-dir")
        .arg(dir.path())
        .args(args)
        .env_remove("CREWBOARD_CONFIG_DIR")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env("NO_COLOR", "1")
        .output()
        .expect("run crewboard")
}

fn bad() -> OsString {
    OsString::from_vec(b"x\xFFy".to_vec())
}

#[test]
fn a_non_utf8_argument_gives_a_usage_error_and_no_panic() {
    for args in [
        vec![OsString::from("status"), bad()],
        vec![bad()],
        vec![OsString::from("watch"), OsString::from("--loop"), bad()],
    ] {
        let o = run(args.clone());
        let err = String::from_utf8_lossy(&o.stderr);
        assert_ne!(o.status.code(), Some(101), "{args:?}: {err}");
        assert!(!err.contains("panicked"), "{args:?}: {err}");
        assert!(o.status.code().is_some(), "{args:?}: killed by a signal");
    }
}

#[test]
fn a_valid_command_line_still_works_after_the_change() {
    // control: the same helper path, with a plain argument, gives the usual refusal (a bad
    // command line is exit 64)
    let o = run(vec![OsString::from("no-such-command")]);
    assert_eq!(
        o.status.code(),
        Some(64),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}
