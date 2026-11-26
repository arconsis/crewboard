//! Repo files through the contents API (`GET repos/<owner>/<name>/contents/<path>`).

use super::repo_path;
use crate::github::{Client, GhError};
use serde_json::Value;

/// One file of a repo, as the contents API gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoFile {
    /// The text, or why there is none, as text that completes "<file> ...".
    pub text: Result<String, String>,
    /// The blob SHA; empty when the answer has none.
    pub sha: String,
}

/// One file of `repo`, at `git_ref` (`None`: the default branch). `path` and `git_ref` go into
/// the URL as they are: a caller encodes them when they may hold special characters. A file
/// that is not there is the HTTP 404 of the client: wrap the call in [`crate::github::optional`]
/// for `None`.
pub fn file(
    client: &Client,
    repo: &str,
    path: &str,
    git_ref: Option<&str>,
) -> Result<RepoFile, GhError> {
    let at = git_ref.map(|r| format!("?ref={r}")).unwrap_or_default();
    let r = client.get(&format!("{}/contents/{path}{at}", repo_path(repo)?))?;
    Ok(RepoFile {
        text: decode_contents(&r.body),
        sha: r
            .body
            .get("sha")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// The names of the entries of a directory of `repo` at `git_ref` (`dir` empty: the root). An
/// answer that is not a list (a file) gives no names. A directory that is not there is the HTTP
/// 404 of the client.
pub fn dir_names(
    client: &Client,
    repo: &str,
    dir: &str,
    git_ref: &str,
) -> Result<Vec<String>, GhError> {
    let path = if dir.is_empty() {
        format!("{}/contents?ref={git_ref}", repo_path(repo)?)
    } else {
        format!("{}/contents/{dir}?ref={git_ref}", repo_path(repo)?)
    };
    let r = client.get(&path)?;
    Ok(r.body
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| e.get("name").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

/// The text of a file from the answer of the contents API: base64 in lines, UTF-8 inside. The
/// one decoder for every read of a repo file. `Err` holds the reason as text that completes
/// "<file> ...": no content (a directory, or a file over the 1 MB limit of the API, which
/// answers `encoding: none`), not base64, or not UTF-8.
fn decode_contents(body: &Value) -> Result<String, String> {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    let content = body.get("content").and_then(Value::as_str);
    let Some(b64) = content.filter(|_| {
        body.get("encoding")
            .and_then(Value::as_str)
            .is_none_or(|e| e == "base64")
    }) else {
        return Err("has no content".to_string());
    };
    let clean: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = STANDARD
        .decode(clean)
        .map_err(|e| format!("is not base64: {e}"))?;
    String::from_utf8(bytes).map_err(|e| format!("is not UTF-8: {e}"))
}
