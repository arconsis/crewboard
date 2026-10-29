//! Bounded paging of REST lists: the one place that follows `Link: rel="next"`.
//!
//! Every caller that reads more than one page goes through here, so the bound is the same
//! everywhere ([`MAX_PAGES`]) and a cut list is a fact the caller sees ([`Paged::truncated`]),
//! never a silent loss. A caller that needs the whole list calls [`Paged::complete`], which turns
//! a cut list into an error.

use super::{Client, GhError, Rest};
use serde_json::Value;

/// The most pages one list read follows (100 items each, so 5000 items).
pub const MAX_PAGES: usize = 50;

/// The pages that were read, and whether more were left.
#[derive(Debug)]
pub struct Paged {
    pub pages: Vec<Rest>,
    /// A next page existed after the last one read: the list is cut at `max_pages`.
    pub truncated: bool,
}

/// Read `path` and follow the next links, at most `max_pages` pages in all (at least 1).
pub fn paged(client: &Client, path: &str, max_pages: usize) -> Result<Paged, GhError> {
    paged_from(client, client.get(path)?, max_pages)
}

/// Like [`paged`], for a first page that the caller read already (for example a conditional
/// GET). Later pages are read without the ETag.
pub fn paged_from(client: &Client, first: Rest, max_pages: usize) -> Result<Paged, GhError> {
    let mut next = first.link_next.clone();
    let mut pages = vec![first];
    while pages.len() < max_pages.max(1) {
        let Some(url) = next.take() else {
            break;
        };
        let page = client.get(&url)?;
        next = page.link_next.clone();
        pages.push(page);
    }
    Ok(Paged {
        pages,
        truncated: next.is_some(),
    })
}

impl Paged {
    /// The list as it is, or an error `"<what> have more than N pages"` when it is cut.
    pub fn complete(self, what: &str) -> Result<Paged, GhError> {
        if self.truncated {
            return Err(GhError::Shape(format!(
                "{what} have more than {} pages",
                self.pages.len()
            )));
        }
        Ok(self)
    }

    /// The items of all pages in order. A page that is not a JSON array is a shape error.
    pub fn items(&self, what: &str) -> Result<Vec<&Value>, GhError> {
        let mut out = Vec::new();
        for page in &self.pages {
            let list = page
                .body
                .as_array()
                .ok_or_else(|| GhError::Shape(format!("{what} is not an array")))?;
            out.extend(list);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page(body: Value, next: bool) -> Rest {
        Rest {
            etag: None,
            link_next: next.then(|| "next".to_string()),
            body,
        }
    }

    #[test]
    fn a_cut_list_is_an_error_when_the_caller_needs_all_of_it() {
        let cut = Paged {
            pages: vec![page(json!([1]), true)],
            truncated: true,
        };
        let e = cut.complete("issues").unwrap_err().to_string();
        assert!(e.contains("issues have more than 1 pages"), "{e}");
    }

    #[test]
    fn items_join_the_pages_in_order_and_refuse_a_non_array() {
        let p = Paged {
            pages: vec![page(json!([1, 2]), false), page(json!([3]), false)],
            truncated: false,
        };
        let all: Vec<_> = p
            .items("list")
            .unwrap()
            .into_iter()
            .map(|v| v.as_i64().unwrap())
            .collect();
        assert_eq!(all, [1, 2, 3]);
        let bad = Paged {
            pages: vec![page(json!({"a": 1}), false)],
            truncated: false,
        };
        assert!(
            bad.items("list")
                .unwrap_err()
                .to_string()
                .contains("list is not an array")
        );
    }
}
