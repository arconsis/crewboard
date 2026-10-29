//! One look at the lock directory. All disk reads of the stale judge happen here, so the judge
//! itself is a pure function of a `Snapshot`, the time and the process table.

use super::fsmeta::mtime_epoch;
use super::owner;
use std::path::Path;

/// What the `owner` entry is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerState {
    /// No `owner` entry.
    Absent,
    /// A file of 0 bytes: counts as no owner file.
    Empty,
    /// A valid owner file (content kept).
    Valid(String),
    /// An entry that is not empty but not valid: wrong content, or not a regular file
    /// (`content` is `None` then).
    Invalid { content: Option<String> },
}

/// One entry of the directory that is not the owner file of a taker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Foreign {
    pub name: String,
    pub regular: bool,
    /// The first 4096 bytes of a regular file.
    pub head: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub exists: bool,
    pub owner: OwnerState,
    /// An invalid owner file first, then the other names sorted by bytes. Never
    /// `owner` (when valid or empty) and never `owner.tmp.*` (the temp file of a taker).
    pub foreign: Vec<Foreign>,
    pub dir_mtime: Option<i64>,
    pub owner_mtime: Option<i64>,
}

fn head_of(path: &Path) -> String {
    use std::io::Read;
    let mut buf = vec![0u8; 4096];
    let n = std::fs::File::open(path)
        .and_then(|mut f| f.read(&mut buf))
        .unwrap_or(0);
    String::from_utf8_lossy(&buf[..n]).to_string()
}

impl Snapshot {
    pub fn read(dir: &Path) -> Snapshot {
        if !dir.is_dir() {
            return Snapshot {
                exists: false,
                owner: OwnerState::Absent,
                foreign: Vec::new(),
                dir_mtime: None,
                owner_mtime: None,
            };
        }
        let op = dir.join("owner");
        let owner = match std::fs::metadata(&op) {
            Err(_) => OwnerState::Absent,
            Ok(m) if m.is_dir() => OwnerState::Invalid { content: None },
            Ok(m) if m.len() == 0 => OwnerState::Empty,
            Ok(m) if m.is_file() => {
                let c = std::fs::read(&op)
                    .map(|b| String::from_utf8_lossy(&b).to_string())
                    .unwrap_or_default();
                if owner::is_valid(&c) {
                    OwnerState::Valid(c)
                } else {
                    OwnerState::Invalid { content: Some(c) }
                }
            }
            // a device, a socket: not a regular file
            Ok(_) => OwnerState::Invalid { content: None },
        };
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| n != "owner" && !n.starts_with("owner.tmp."))
                    .collect()
            })
            .unwrap_or_default();
        names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        let mut foreign = Vec::new();
        if let OwnerState::Invalid { content } = &owner {
            foreign.push(Foreign {
                name: "owner".to_string(),
                regular: content.is_some(),
                head: content
                    .as_deref()
                    .map(|c| c[..floor_boundary(c, 4096)].to_string())
                    .unwrap_or_default(),
            });
        }
        for n in names {
            let p = dir.join(&n);
            let regular = std::fs::metadata(&p).is_ok_and(|m| m.is_file());
            foreign.push(Foreign {
                head: if regular { head_of(&p) } else { String::new() },
                name: n,
                regular,
            });
        }
        Snapshot {
            exists: true,
            owner,
            foreign,
            dir_mtime: mtime_epoch(dir),
            owner_mtime: mtime_epoch(&op),
        }
    }

    /// Is an `owner` entry there that is not empty (`[ -s owner ]`)?
    pub fn owner_present(&self) -> bool {
        matches!(
            self.owner,
            OwnerState::Valid(_) | OwnerState::Invalid { .. }
        )
    }

    /// The text of a regular, non-empty owner file, valid or not.
    pub fn owner_text(&self) -> Option<&str> {
        match &self.owner {
            OwnerState::Valid(c) => Some(c),
            OwnerState::Invalid { content } => content.as_deref(),
            _ => None,
        }
    }

    pub fn owner_field(&self, key: &str) -> Option<&str> {
        self.owner_text().and_then(|c| owner::field(c, key))
    }

    pub fn is_valid(&self) -> bool {
        matches!(self.owner, OwnerState::Valid(_))
    }

    pub fn is_invalid(&self) -> bool {
        matches!(self.owner, OwnerState::Invalid { .. })
    }
}

fn floor_boundary(s: &str, max: usize) -> usize {
    let mut i = s.len().min(max);
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (n, c) in files {
            std::fs::write(d.path().join(n), c).unwrap();
        }
        d
    }

    #[test]
    fn a_missing_dir_does_not_exist_and_an_empty_one_is_empty() {
        let d = tempfile::tempdir().unwrap();
        assert!(!Snapshot::read(&d.path().join("nope")).exists);
        let s = Snapshot::read(d.path());
        assert!(s.exists && s.owner == OwnerState::Absent && s.foreign.is_empty());
    }

    #[test]
    fn f6_owner_tmp_files_are_not_foreign() {
        let d = dir_with(&[("owner.tmp.123", "agent=x\n")]);
        let s = Snapshot::read(d.path());
        assert!(s.foreign.is_empty());
        // the other direction: any other name is foreign
        let d = dir_with(&[("owner.tmp.123", "x"), ("info", "i")]);
        let s = Snapshot::read(d.path());
        assert_eq!(s.foreign.len(), 1);
        assert_eq!(s.foreign[0].name, "info");
    }

    #[test]
    fn foreign_entries_are_sorted_by_bytes_and_an_invalid_owner_comes_first() {
        let d = dir_with(&[
            ("zz", "1"),
            ("B", "2"),
            ("a", "3"),
            (".hidden", "4"),
            ("owner", "agent-a@host-b\n"),
        ]);
        let s = Snapshot::read(d.path());
        let names: Vec<_> = s.foreign.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["owner", ".hidden", "B", "a", "zz"]);
        assert!(s.is_invalid());
    }

    #[test]
    fn o6_an_owner_that_is_a_directory_is_invalid_and_not_regular() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("owner")).unwrap();
        let s = Snapshot::read(d.path());
        assert!(s.is_invalid() && s.owner_present());
        assert!(!s.foreign[0].regular);
        assert_eq!(s.owner_text(), None);
    }

    #[test]
    fn an_empty_owner_file_counts_as_no_owner() {
        let d = dir_with(&[("owner", "")]);
        let s = Snapshot::read(d.path());
        assert_eq!(s.owner, OwnerState::Empty);
        assert!(!s.owner_present() && s.foreign.is_empty());
    }

    #[test]
    fn a_valid_owner_gives_its_fields() {
        let d = dir_with(&[("owner", "agent=a@x\nhost=h\npid=4\nstarted_epoch=9\n")]);
        let s = Snapshot::read(d.path());
        assert!(s.is_valid());
        assert_eq!(s.owner_field("host"), Some("h"));
        assert_eq!(s.owner_field("nope"), None);
    }
}
