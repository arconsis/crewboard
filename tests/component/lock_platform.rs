//! The platform rule of `crewboard lock`: it needs a Unix host. On Windows it refuses with the rule
//! `unix-only` (exit 2) before it touches a file; on Unix it does not refuse.

use std::process::Command;

#[test]
fn lock_refuses_with_unix_only_on_windows_and_runs_on_unix() {
    let d = tempfile::tempdir().unwrap();
    let lock = d.path().join("l");
    // an empty process table: a CI job of the real host must not change the answer
    let ps = d.path().join("ps.txt");
    std::fs::write(&ps, "").unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_crewboard"))
        .env("HOST_LOCK_HOST", "hostA")
        .env("HOST_LOCK_PS_CMD", format!("cat {}", ps.display()))
        .args(["lock", "status"])
        .arg(&lock)
        .arg("--no-board")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    if cfg!(windows) {
        assert_eq!(o.status.code(), Some(2), "{err}");
        assert!(err.contains("rule=unix-only"), "{err}");
        assert!(!lock.exists());
    } else {
        // a free lock: status says so with exit 1 (the other direction: no refusal)
        assert_eq!(o.status.code(), Some(1), "{err}");
        assert!(!err.contains("unix-only"), "{err}");
        assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "free");
    }
}
