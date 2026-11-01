//! The change time of a path in whole Unix seconds, from `std::fs::metadata`. No `stat`
//! parsing, so the BSD and GNU spellings do not matter.

use std::path::Path;
use std::time::UNIX_EPOCH;

/// `None` when the path does not exist or its time cannot be read.
pub fn mtime_epoch(path: &Path) -> Option<i64> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => Some(d.as_secs() as i64),
        Err(e) => Some(-(e.duration().as_secs() as i64)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    #[test]
    fn m1_the_epoch_is_exact() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("f");
        std::fs::write(&f, "x").unwrap();
        let want = 1_767_367_845u64; // 2026-01-02T15:30:45Z
        let h = std::fs::File::options().write(true).open(&f).unwrap();
        h.set_modified(UNIX_EPOCH + Duration::from_secs(want))
            .unwrap();
        assert_eq!(mtime_epoch(&f), Some(want as i64));
        // the other direction: a file of another time is not that epoch
        h.set_modified(SystemTime::now()).unwrap();
        assert_ne!(mtime_epoch(&f), Some(want as i64));
    }

    #[test]
    fn m3_a_missing_path_is_none() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(mtime_epoch(&d.path().join("nope")), None);
    }
}
