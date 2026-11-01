//! Typed REST calls of the endpoints that the commands use. Each function builds the path,
//! reads the pages through [`super::paging`] and parses the answer into a type of its module,
//! so a command never formats a REST path for these endpoints.
//!
//! A list read is one of two kinds, and its name and doc say which: the whole list (every
//! page; a list cut at [`MAX_PAGES`](super::paging::MAX_PAGES) is an error, or a [`Listed`]
//! with `truncated` where the caller decides), or one page on purpose (a sample, or a search
//! where any hit will do).

pub mod pulls;

use crate::github::GhError;

/// A list read with a bound: the items, and whether more were left after the last page read.
#[derive(Debug, Clone, PartialEq)]
pub struct Listed<T> {
    pub items: Vec<T>,
    pub truncated: bool,
}

/// `repos/<owner>/<name>` for `owner/name`.
fn repo_path(repo: &str) -> Result<String, GhError> {
    match repo.split_once('/') {
        Some((o, n)) if !o.is_empty() && !n.is_empty() && !n.contains('/') => {
            Ok(format!("repos/{o}/{n}"))
        }
        _ => Err(GhError::Shape(format!("'{repo}' is not owner/repo"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_path_takes_owner_and_name_only() {
        assert_eq!(repo_path("o/r").unwrap(), "repos/o/r");
        for bad in ["o", "/r", "o/", "o/r/x"] {
            assert!(matches!(repo_path(bad), Err(GhError::Shape(_))), "{bad}");
        }
    }
}
