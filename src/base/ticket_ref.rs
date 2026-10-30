//! One ticket: an issue in a repo of the initiative (spec/tickets).

use std::fmt;

/// A ticket by its repo (`owner/name`) and issue number. `Display` is `owner/name#n`, the form of
/// the CLI, the messages and the state file. Two refs of one ticket can differ in the case of the
/// repo (GitHub names are case-insensitive): compare them with [`TicketRef::key`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TicketRef {
    /// `owner/name`, as GitHub or the user wrote it.
    pub repo: String,
    pub issue: u64,
}

impl TicketRef {
    pub fn new(repo: impl Into<String>, issue: u64) -> Self {
        TicketRef {
            repo: repo.into(),
            issue,
        }
    }

    /// Parse `owner/name#n`. `None` for any other text.
    pub fn parse(s: &str) -> Option<Self> {
        let (repo, n) = s.rsplit_once('#')?;
        let (owner, name) = repo.split_once('/')?;
        if owner.is_empty() || name.is_empty() || name.contains('/') {
            return None;
        }
        Some(TicketRef::new(repo, n.parse().ok()?))
    }

    /// The key of the ticket for comparisons and for the note memory of the state file:
    /// `owner/name#n` with the repo in lower case, so `Org/Repo#7` and `org/repo#7` are one
    /// ticket.
    pub fn key(&self) -> String {
        format!("{}#{}", self.repo.to_ascii_lowercase(), self.issue)
    }

    /// The owner and the name of the repo; `None` when the repo has no `/`.
    pub fn owner_and_name(&self) -> Option<(&str, &str)> {
        self.repo.split_once('/')
    }
}

impl fmt::Display for TicketRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.repo, self.issue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_parse_are_inverse() {
        let t = TicketRef::new("Org/Repo", 7);
        assert_eq!(t.to_string(), "Org/Repo#7");
        assert_eq!(TicketRef::parse("Org/Repo#7"), Some(t));
        for bad in ["7", "o/r", "o/r#", "o/r#x", "/r#1", "o/#1", "o/r/x#1", "#1"] {
            assert_eq!(TicketRef::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_key_ignores_the_case_of_the_repo_and_the_display_keeps_it() {
        let a = TicketRef::new("Org/Repo", 7);
        let b = TicketRef::new("org/repo", 7);
        assert_eq!(a.key(), "org/repo#7");
        assert_eq!(a.key(), b.key());
        assert_ne!(a, b, "equality is exact; `key` compares tickets");
        assert_ne!(a.key(), TicketRef::new("org/repo", 8).key());
        assert_ne!(a.key(), TicketRef::new("org/other", 7).key());
        assert_eq!(a.owner_and_name(), Some(("Org", "Repo")));
    }
}
