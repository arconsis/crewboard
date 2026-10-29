//! GitHub client: GraphQL and REST over HTTPS (spec/events#polling-and-rate-limits).
//!
//! Rules built in here:
//!
//! - A GraphQL answer with HTTP 200 and an `errors` array is a failure.
//!   `extensions.warnings` is not.
//! - GitHub's "temporary conflict" on a write is retried at most 3 times with backoff,
//!   then it is an error (exit 3).
//! - Conditional requests (ETag) for feeds; a 304 is a distinct result, never an error.
//! - The token comes from the env file of the agent (`agent.env`, `--env-file`), else from
//!   `GH_TOKEN` / `GITHUB_TOKEN`, else from `gh auth token` (see `agent_env` for the order). It
//!   is never stored in a config file, never printed, and `Debug` redacts it.

pub(crate) mod paging;
pub(crate) mod projects;
mod token;

pub use token::{TEST_GH_AUTH_TOKEN_ENV, resolve_owner_token, resolve_token};

use reqwest::blocking::Client as Http;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, USER_AGENT};
use serde_json::{Value, json};
use std::fmt;
use std::time::Duration;

/// An API token. Never shown in `Debug` or `Display`.
#[derive(Clone)]
pub struct Token(String);

impl Token {
    pub fn new(s: impl Into<String>) -> Self {
        Token(s.into())
    }
    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

// re-export; new code imports from base
pub use crate::base::GhError;

/// The `login` of a `GET /user` answer. A missing or empty login is a shape error: a check
/// that cannot read the login must fail, never pass.
pub fn parse_viewer_login(body: &Value) -> Result<String, GhError> {
    match body.get("login").and_then(Value::as_str) {
        Some(l) if !l.is_empty() => Ok(l.to_string()),
        _ => Err(GhError::Shape("GET /user has no login".into())),
    }
}

/// HTTP 422 whose error list says `already_exists` (a duplicate label). Any other 422 is a
/// real validation failure and must not be mistaken for "reuse".
pub fn is_already_exists(e: &GhError) -> bool {
    matches!(e, GhError::Http { status: 422, message, .. } if message.contains("already_exists"))
}

/// True if a GraphQL/REST message is GitHub's transient write conflict.
pub fn is_temporary_conflict(text: &str) -> bool {
    text.contains("temporary conflict")
}

/// Judge one GraphQL HTTP answer. Returns the `data` object.
///
/// Pure, so it can be tested without a network.
pub fn parse_graphql(status: u16, body: &str) -> Result<Value, GhError> {
    if status == 401 {
        return Err(GhError::Auth(format!("HTTP 401: {}", snippet(body))));
    }
    if status != 200 {
        return Err(GhError::Http {
            status,
            message: snippet(body),
            accepted_permissions: None,
        });
    }
    let v: Value = serde_json::from_str(body).map_err(|e| {
        GhError::Shape(format!("GraphQL body is not JSON ({e}): {}", snippet(body)))
    })?;
    let Some(obj) = v.as_object() else {
        return Err(GhError::Shape(format!(
            "GraphQL body is not an object: {}",
            snippet(body)
        )));
    };
    if let Some(errs) = obj.get("errors") {
        let empty = errs.as_array().is_some_and(Vec::is_empty) || errs.is_null();
        if !empty {
            return Err(graphql_errors(errs));
        }
    }
    match obj.get("data") {
        Some(d) if d.is_object() => Ok(d.clone()),
        _ => Err(GhError::Shape("GraphQL answer has no `data` object".into())),
    }
}

/// How many distinct GraphQL error messages are printed before "and N more".
const MAX_GRAPHQL_MESSAGES: usize = 3;

/// Turn the `errors` value of a GraphQL answer into one error.
///
/// GitHub answers a token without a needed scope with one `INSUFFICIENT_SCOPES` error per
/// field, each with the same message. Those become `GhError::InsufficientScopes`. Other errors
/// are deduplicated (order kept) and capped at `MAX_GRAPHQL_MESSAGES`.
fn graphql_errors(errs: &Value) -> GhError {
    let Some(list) = errs.as_array() else {
        return GhError::GraphQl(errs.to_string());
    };
    let mut needed: Vec<String> = Vec::new();
    let mut granted: Vec<String> = Vec::new();
    let mut scope_error = false;
    let mut others: Vec<String> = Vec::new();
    for e in list {
        let msg = e
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("(no message)");
        let typed = e.get("type").and_then(Value::as_str) == Some("INSUFFICIENT_SCOPES");
        if let Some((n, g)) = typed.then(|| parse_scope_message(msg)).flatten() {
            scope_error = true;
            for s in n {
                if !needed.contains(&s) {
                    needed.push(s);
                }
            }
            for s in g {
                if !granted.contains(&s) {
                    granted.push(s);
                }
            }
            continue;
        }
        if !others.iter().any(|m| m == msg) {
            others.push(msg.to_string());
        }
    }
    if scope_error {
        let other = cap_messages(others);
        return GhError::InsufficientScopes {
            needed,
            granted,
            other,
        };
    }
    GhError::GraphQl(cap_messages(others).join("; "))
}

/// The first `MAX_GRAPHQL_MESSAGES` messages, then one entry "and N more".
fn cap_messages(mut msgs: Vec<String>) -> Vec<String> {
    if msgs.len() > MAX_GRAPHQL_MESSAGES {
        let more = msgs.len() - MAX_GRAPHQL_MESSAGES;
        msgs.truncate(MAX_GRAPHQL_MESSAGES);
        msgs.push(format!("and {more} more"));
    }
    msgs
}

/// The quoted names of the first `[...]` list after `marker`.
fn quoted_list(text: &str, marker: &str) -> Option<Vec<String>> {
    let rest = &text[text.find(marker)? + marker.len()..];
    let open = rest.find('[')?;
    let close = open + rest[open..].find(']')?;
    Some(
        rest[open + 1..close]
            .split(',')
            .map(|s| s.trim().trim_matches(|c| c == '\'' || c == '"').to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    )
}

/// `(needed, granted)` from the message of an `INSUFFICIENT_SCOPES` error:
/// "... requires one of the following scopes: ['read:project'], but your token has only been
/// granted the: ['gist', 'repo'] scopes. ...".
fn parse_scope_message(msg: &str) -> Option<(Vec<String>, Vec<String>)> {
    let needed = quoted_list(msg, "requires one of the following scopes")?;
    let granted = quoted_list(msg, "only been granted the").unwrap_or_default();
    if needed.is_empty() {
        return None;
    }
    Some((needed, granted))
}

fn snippet(s: &str) -> String {
    let t: String = s.chars().take(300).collect();
    t.replace('\n', " ")
}

/// Run `f`; on a temporary conflict, wait and run it again, at most `max_retries` times.
/// `sleep` is a parameter so tests do not wait.
pub fn retry_conflict<T>(
    max_retries: u32,
    mut sleep: impl FnMut(Duration),
    mut f: impl FnMut() -> Result<T, GhError>,
) -> Result<T, GhError> {
    let mut attempt = 0;
    loop {
        match f() {
            Err(GhError::GraphQl(m)) | Err(GhError::Http { message: m, .. })
                if is_temporary_conflict(&m) =>
            {
                if attempt >= max_retries {
                    return Err(GhError::ConflictExhausted(m));
                }
                sleep(Duration::from_millis(500 * (1u64 << attempt)));
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// Attempts for a transient failure (the first try plus two retries).
pub const TRANSIENT_ATTEMPTS: u32 = 3;
/// Waits before the 2nd and 3rd attempt: 7 s in all.
const TRANSIENT_WAITS: [u64; 2] = [2, 5];

/// A failure that may go away: transport errors (connect, send, timeout, reset) and HTTP
/// 502, 503, 504. An auth error, a 4xx, a GraphQL error or a bad shape is never transient.
pub fn is_transient(e: &GhError) -> bool {
    match e {
        GhError::Network(_) => true,
        GhError::Http { status, .. } => matches!(status, 502..=504),
        _ => false,
    }
}

/// Run `f` up to `attempts` times while it fails with a transient error. `sleep` is a
/// parameter so tests do not wait. When the attempts are used up the error keeps its kind
/// (exit 3) and its text names the number of attempts.
///
/// ONLY for calls that a repeat cannot double-apply: reads, and idempotent writes.
pub fn retry_transient<T>(
    attempts: u32,
    mut sleep: impl FnMut(Duration),
    mut f: impl FnMut() -> Result<T, GhError>,
) -> Result<T, GhError> {
    let mut n = 0;
    loop {
        n += 1;
        match f() {
            Err(e) if is_transient(&e) => {
                if n >= attempts {
                    return Err(with_attempts(e, n));
                }
                let w = TRANSIENT_WAITS[(n as usize - 1).min(TRANSIENT_WAITS.len() - 1)];
                eprintln!(
                    "crewboard: transient GitHub failure, attempt {n} of {attempts}: {e}; retrying in {w} s"
                );
                sleep(Duration::from_secs(w));
            }
            other => return other,
        }
    }
}

/// The error with "(after N attempts)" in its text; the kind stays.
pub fn with_attempts(e: GhError, n: u32) -> GhError {
    if n <= 1 {
        return e;
    }
    match e {
        GhError::Network(m) => GhError::Network(format!("{m} (after {n} attempts)")),
        GhError::Http {
            status,
            message,
            accepted_permissions,
        } => GhError::Http {
            status,
            message: format!("{message} (after {n} attempts)"),
            accepted_permissions,
        },
        other => other,
    }
}

/// Mutations that give the same result when sent twice (a field set, a collaborator role,
/// adding an issue that is already on the board, a pin).
const IDEMPOTENT_MUTATIONS: &[&str] = &[
    "updateProjectV2ItemFieldValue",
    "clearProjectV2ItemFieldValue",
    "updateProjectV2Field",
    "updateProjectV2DraftIssue",
    "updateProjectV2Collaborators",
    "addProjectV2ItemById",
    "pinIssue",
];

/// May this GraphQL document be sent again after a transient failure? Every query: yes. A
/// mutation: only if every mutation field it calls is idempotent. An unknown field counts
/// as not repeatable.
pub fn graphql_is_repeatable(query: &str) -> bool {
    let q = query.trim_start();
    if !q.starts_with("mutation") {
        return true;
    }
    let names: Vec<String> = q
        .match_indices("(input:")
        .map(|(i, _)| {
            q[..i]
                .chars()
                .rev()
                .take_while(|c| c.is_alphanumeric())
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        })
        .collect();
    !names.is_empty()
        && names
            .iter()
            .all(|n| IDEMPOTENT_MUTATIONS.contains(&n.as_str()))
}

/// A REST answer.
#[derive(Debug, Clone)]
pub struct Rest {
    pub etag: Option<String>,
    pub link_next: Option<String>,
    pub body: Value,
}

/// Answer of a conditional GET.
#[derive(Debug, Clone)]
pub enum Conditional {
    /// HTTP 304: nothing changed. Costs no primary rate limit.
    NotModified,
    Modified(Rest),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    pub remaining: u64,
    pub limit: u64,
    pub reset: i64,
}

/// The environment variable that moves the API to a local server (tests with a fake GitHub).
pub const API_BASE_ENV: &str = "CREWBOARD_API_BASE";
const DEFAULT_API_BASE: &str = "https://api.github.com";

/// Check a `CREWBOARD_API_BASE` value. Only `http://` with the host `127.0.0.1`, `localhost`
/// or `[::1]` (and an optional port) is accepted, so the token can never go to a foreign
/// host. Anything else is a configuration error (exit 4).
pub fn parse_api_base(raw: &str) -> Result<String, GhError> {
    let bad = |why: &str| {
        GhError::Auth(format!(
            "{API_BASE_ENV}={raw:?} is refused: {why}. Only http://127.0.0.1, http://localhost or http://[::1] (optional port) is allowed, so the token never leaves this machine"
        ))
    };
    let v = raw.trim();
    let Some(rest) = v.strip_prefix("http://") else {
        return Err(bad("the scheme must be http"));
    };
    let rest = rest.trim_end_matches('/');
    if rest.contains(['/', '?', '#', '@', '\\']) || rest.chars().any(char::is_whitespace) {
        return Err(bad("no path, query, user or fragment is allowed"));
    }
    let (host, port) = if let Some(r) = rest.strip_prefix('[') {
        match r.split_once(']') {
            Some((h, p)) => (
                format!("[{h}]"),
                p.strip_prefix(':').or(p.is_empty().then_some("")),
            ),
            None => return Err(bad("bad IPv6 host")),
        }
    } else {
        match rest.split_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (rest.to_string(), Some("")),
        }
    };
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]") {
        return Err(bad("the host is not a loopback host"));
    }
    match port {
        Some(p) if p.is_empty() || (p.len() <= 5 && p.chars().all(|c| c.is_ascii_digit())) => {}
        _ => return Err(bad("bad port")),
    }
    Ok(format!("http://{rest}"))
}

pub struct Client {
    http: Http,
    token: Token,
    base: String,
    /// Login of the token's account, read once per run (`GET /user`).
    viewer: std::sync::OnceLock<String>,
    /// The `x-oauth-scopes` header of the last REST answer that had one. Only a classic token
    /// gets it; a fine-grained token never does (`doctor --setup`).
    oauth_scopes: std::sync::Mutex<Option<String>>,
    /// Longest single wait of a retry or rate-limit backoff. `None` in production. `Client::new`
    /// sets it only when `CREWBOARD_API_BASE` points at a loopback fake, so a test that makes
    /// the fake answer 502 or 429 does not wait seconds for real.
    sleep_cap: Option<Duration>,
}

/// The cap of one backoff wait while `CREWBOARD_API_BASE` is set (the test fake).
const FAKE_SLEEP_CAP: Duration = Duration::from_millis(50);

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Client {{ base: {}, token: <redacted> }}", self.base)
    }
}

fn next_link(headers: &HeaderMap) -> Option<String> {
    let link = headers.get("link")?.to_str().ok()?;
    link.split(',').find_map(|part| {
        let (url, rel) = part.split_once(';')?;
        if rel.contains("rel=\"next\"") {
            Some(
                url.trim()
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string(),
            )
        } else {
            None
        }
    })
}

/// Add the proxy settings of the env file that the process environment does not have.
fn with_file_proxies(
    mut b: reqwest::blocking::ClientBuilder,
    env: &crate::agent_env::AgentEnv,
) -> Result<reqwest::blocking::ClientBuilder, GhError> {
    let entries = env.proxy_entries(&crate::agent_env::caller_has);
    let no_proxy = entries
        .iter()
        .find(|(k, _)| k == "no_proxy")
        .and_then(|(_, v)| reqwest::NoProxy::from_string(v));
    for (kind, url) in entries.iter().filter(|(k, _)| k != "no_proxy") {
        let bad = |_| GhError::Network(format!("{kind} in the env file is not a proxy URL"));
        let p = match kind.as_str() {
            "https_proxy" => reqwest::Proxy::https(url),
            "http_proxy" => reqwest::Proxy::http(url),
            _ => reqwest::Proxy::all(url),
        }
        .map_err(bad)?;
        b = b.proxy(p.no_proxy(no_proxy.clone()));
    }
    Ok(b)
}

impl Client {
    pub fn new(token: Token) -> Result<Self, GhError> {
        let (base, fake) = match crate::agent_env::var(API_BASE_ENV) {
            Some(v) if !v.trim().is_empty() => (parse_api_base(&v)?, true),
            _ => (DEFAULT_API_BASE.to_string(), false),
        };
        let mut client = Client::with_base(token, base)?;
        if fake {
            client.sleep_cap = Some(FAKE_SLEEP_CAP);
        }
        Ok(client)
    }

    /// A client for the API base `raw_base`, checked like `CREWBOARD_API_BASE` (loopback only).
    /// Tests that call the library use it, so they do not change the process environment.
    pub fn for_base(token: Token, raw_base: &str) -> Result<Self, GhError> {
        Client::with_base(token, parse_api_base(raw_base)?)
    }

    /// Wait `d`, at most `sleep_cap` (see there). The retry functions take this as their
    /// `sleep` parameter.
    fn sleep(&self, d: Duration) {
        crate::clock::pause(self.sleep_cap.map_or(d, |cap| d.min(cap)));
    }

    fn with_base(token: Token, base: String) -> Result<Self, GhError> {
        let mut builder = Http::builder().timeout(Duration::from_secs(40));
        // reqwest reads the proxy variables of the process itself; the ones of the env file are
        // not in the process environment, so they are given here
        if let Some(env) = crate::agent_env::current() {
            builder = with_file_proxies(builder, env)?;
        }
        let http = builder
            .build()
            .map_err(|e| GhError::Network(e.to_string()))?;
        Ok(Client {
            http,
            token,
            base,
            viewer: std::sync::OnceLock::new(),
            oauth_scopes: std::sync::Mutex::new(None),
            sleep_cap: None,
        })
    }

    /// The scopes of a classic token, from the `x-oauth-scopes` header of the last REST answer
    /// (for example `project, repo`). `None`: no answer had the header yet, or the token is
    /// fine-grained (GitHub shows no scopes for it).
    pub fn oauth_scopes(&self) -> Option<String> {
        self.oauth_scopes.lock().ok().and_then(|g| g.clone())
    }

    /// The login of the account the token belongs to (`GET /user`), cached for the run.
    pub fn viewer_login(&self) -> Result<String, GhError> {
        if let Some(v) = self.viewer.get() {
            return Ok(v.clone());
        }
        let r = self.get("user")?;
        let login = parse_viewer_login(&r.body)?;
        Ok(self.viewer.get_or_init(|| login).clone())
    }

    fn request(&self, method: reqwest::Method, url: &str) -> reqwest::blocking::RequestBuilder {
        self.http
            .request(method, url)
            .header(AUTHORIZATION, format!("Bearer {}", self.token.expose()))
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header(USER_AGENT, concat!("crewboard/", env!("CARGO_PKG_VERSION")))
    }

    /// The full URL for `path`. A relative path goes under the base. An absolute URL is
    /// followed only when it starts with `<base>/`. Any other absolute URL is an error that
    /// names its host, so the token never goes to a host that an answer names.
    fn url(&self, path: &str) -> Result<String, GhError> {
        let lower = path.get(..8).unwrap_or(path).to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return Ok(format!("{}/{}", self.base, path.trim_start_matches('/')));
        }
        if path.starts_with(&format!("{}/", self.base)) {
            return Ok(path.to_string());
        }
        let host = reqwest::Url::parse(path)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| "(unreadable host)".to_string());
        Err(GhError::Shape(format!(
            "refused to follow an absolute URL on host {host}: only URLs under {}/ are followed, and no request was sent",
            self.base
        )))
    }

    /// GraphQL call. Returns the `data` object. Retries a temporary conflict 3 times.
    pub fn graphql(&self, query: &str, variables: Value) -> Result<Value, GhError> {
        if graphql_is_repeatable(query) {
            retry_transient(
                TRANSIENT_ATTEMPTS,
                |d| self.sleep(d),
                || self.graphql_once(query, &variables),
            )
        } else {
            self.graphql_once(query, &variables)
        }
    }

    fn graphql_once(&self, query: &str, variables: &Value) -> Result<Value, GhError> {
        retry_conflict(
            3,
            |d| self.sleep(d),
            || {
                let body = json!({"query": query, "variables": variables});
                let url = self.url("graphql")?;
                let resp =
                    self.send_limited(|| self.request(reqwest::Method::POST, &url).json(&body))?;
                let status = resp.status().as_u16();
                let text = resp
                    .text()
                    .map_err(|e| GhError::Network(format!("reading GraphQL body: {e}")))?;
                parse_graphql(status, &text)
            },
        )
    }

    /// Send, and wait out a rate limit answer (at most 3 times, at most 60 s each).
    fn send_limited(
        &self,
        build: impl Fn() -> reqwest::blocking::RequestBuilder,
    ) -> Result<reqwest::blocking::Response, GhError> {
        let mut tries = 0;
        loop {
            let resp = build()
                .send()
                .map_err(|e| GhError::Network(e.without_url().to_string()))?;
            let status = resp.status().as_u16();
            if status == 403 || status == 429 {
                let headers = resp.headers().clone();
                let retry_after = headers
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok());
                let remaining_zero = headers
                    .get("x-ratelimit-remaining")
                    .and_then(|v| v.to_str().ok())
                    == Some("0");
                if retry_after.is_some() || remaining_zero {
                    let wait = retry_after.unwrap_or_else(|| {
                        let reset = headers
                            .get("x-ratelimit-reset")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.parse::<i64>().ok())
                            .unwrap_or(0);
                        (reset - crate::clock::now_epoch()).clamp(1, 3600) as u64
                    });
                    if tries < 3 && wait <= 60 {
                        tries += 1;
                        self.sleep(Duration::from_secs(wait + 1));
                        continue;
                    }
                    return Err(GhError::Http {
                        status,
                        message: format!("rate limited; retry after {wait} s"),
                        accepted_permissions: None,
                    });
                }
            }
            return Ok(resp);
        }
    }

    fn rest_inner(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        etag: Option<&str>,
    ) -> Result<Conditional, GhError> {
        // A GET can always be repeated. A write is sent once (comments: `feed::post_comment`).
        if method == reqwest::Method::GET {
            retry_transient(
                TRANSIENT_ATTEMPTS,
                |d| self.sleep(d),
                || self.rest_once(&method, path, body, etag),
            )
        } else {
            self.rest_once(&method, path, body, etag)
        }
    }

    fn rest_once(
        &self,
        method: &reqwest::Method,
        path: &str,
        body: Option<&Value>,
        etag: Option<&str>,
    ) -> Result<Conditional, GhError> {
        retry_conflict(
            3,
            |d| self.sleep(d),
            || {
                let url = self.url(path)?;
                let resp = self.send_limited(|| {
                    let mut r = self.request(method.clone(), &url);
                    if let Some(b) = body {
                        r = r.json(b);
                    }
                    if let Some(e) = etag {
                        r = r.header("If-None-Match", e);
                    }
                    r
                })?;
                let status = resp.status().as_u16();
                if status == 304 {
                    return Ok(Conditional::NotModified);
                }
                let etag = resp
                    .headers()
                    .get("etag")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let link_next = next_link(resp.headers());
                if let Some(v) = resp
                    .headers()
                    .get("x-oauth-scopes")
                    .and_then(|v| v.to_str().ok())
                    && let Ok(mut g) = self.oauth_scopes.lock()
                {
                    *g = Some(v.trim().to_string());
                }
                let accepted_permissions = resp
                    .headers()
                    .get("x-accepted-github-permissions")
                    .and_then(|v| v.to_str().ok())
                    .map(|v| snippet(v.trim()))
                    .filter(|v| !v.is_empty());
                let text = resp
                    .text()
                    .map_err(|e| GhError::Network(format!("reading body: {e}")))?;
                if status >= 400 {
                    let message = serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
                        .unwrap_or_else(|| snippet(&text));
                    let detail = serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|v| v.get("errors").map(|e| e.to_string()))
                        .map(|e| format!(" {}", snippet(&e)))
                        .unwrap_or_default();
                    if status == 401 {
                        return Err(GhError::Auth(message));
                    }
                    return Err(GhError::Http {
                        status,
                        message: format!("{message}{detail}"),
                        accepted_permissions,
                    });
                }
                let body = if text.trim().is_empty() {
                    Value::Null
                } else {
                    serde_json::from_str(&text)
                        .map_err(|e| GhError::Shape(format!("REST body is not JSON: {e}")))?
                };
                Ok(Conditional::Modified(Rest {
                    etag,
                    link_next,
                    body,
                }))
            },
        )
    }

    pub fn get(&self, path: &str) -> Result<Rest, GhError> {
        match self.rest_inner(reqwest::Method::GET, path, None, None)? {
            Conditional::Modified(r) => Ok(r),
            Conditional::NotModified => Err(GhError::Shape("304 without If-None-Match".into())),
        }
    }

    pub fn get_conditional(&self, path: &str, etag: Option<&str>) -> Result<Conditional, GhError> {
        self.rest_inner(reqwest::Method::GET, path, None, etag)
    }

    pub fn send(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Rest, GhError> {
        let m = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|e| GhError::Shape(e.to_string()))?;
        match self.rest_inner(m, path, body, None)? {
            Conditional::Modified(r) => Ok(r),
            Conditional::NotModified => Err(GhError::Shape("unexpected 304".into())),
        }
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Rest, GhError> {
        self.send("POST", path, Some(body))
    }

    /// `resource` is `core` (REST) or `graphql`.
    pub fn rate_limit_of(&self, resource: &str) -> Result<RateLimit, GhError> {
        let r = self.get("rate_limit")?;
        let core = r
            .body
            .pointer(&format!("/resources/{resource}"))
            .ok_or_else(|| GhError::Shape(format!("rate_limit has no resources.{resource}")))?;
        let n = |k: &str| {
            core.get(k)
                .and_then(Value::as_u64)
                .ok_or_else(|| GhError::Shape(format!("rate_limit core.{k} missing")))
        };
        Ok(RateLimit {
            remaining: n("remaining")?,
            limit: n("limit")?,
            reset: n("reset")? as i64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn graphql_200_with_errors_is_a_failure() {
        let body = r#"{"data":{"x":1},"errors":[{"message":"Could not resolve"}]}"#;
        let e = parse_graphql(200, body).unwrap_err();
        assert_eq!(e, GhError::GraphQl("Could not resolve".into()));
    }

    /// The answer of GitHub to a token without `read:project`: one error per field, one type.
    fn scope_errors_body(n_fields: usize) -> String {
        let msg = "Your token has not been granted the required scopes to execute this query. \
                   The 'projectV2' field requires one of the following scopes: ['read:project'], \
                   but your token has only been granted the: ['gist', 'read:org', 'repo', \
                   'workflow'] scopes. Please modify your token's scopes at: \
                   https://github.com/settings/tokens.";
        let errs: Vec<Value> = (0..n_fields)
            .map(|_| json!({"type": "INSUFFICIENT_SCOPES", "message": msg}))
            .collect();
        json!({"data": null, "errors": errs}).to_string()
    }

    #[test]
    fn insufficient_scopes_are_one_error_with_the_scopes_and_the_fix() {
        let e = parse_graphql(200, &scope_errors_body(10)).unwrap_err();
        assert_eq!(
            e,
            GhError::InsufficientScopes {
                needed: vec!["read:project".into()],
                granted: ["gist", "read:org", "repo", "workflow"]
                    .map(String::from)
                    .to_vec(),
                other: vec![],
            }
        );
        let text = e.to_string();
        assert_eq!(
            text,
            "the GitHub token lacks the scope read:project (it has: gist, read:org, repo, workflow). \
             For a gh login: gh auth refresh -h github.com -s project. \
             Or create a token with that scope."
        );
        assert!(!text.contains('\n') && text.len() < 250, "{text}");
        assert_eq!(
            crate::error::CbError::Network(e).exit(),
            crate::exit::Exit::Config
        );
    }

    #[test]
    fn scopes_of_several_errors_are_joined_without_duplicates() {
        let body = json!({"errors": [
            {"type": "INSUFFICIENT_SCOPES", "message":
                "x requires one of the following scopes: ['read:project', 'project'], but your token has only been granted the: ['repo'] scopes."},
            {"type": "INSUFFICIENT_SCOPES", "message":
                "y requires one of the following scopes: ['project', 'read:org'], but your token has only been granted the: ['repo'] scopes."},
        ]})
        .to_string();
        let e = parse_graphql(200, &body).unwrap_err();
        let GhError::InsufficientScopes {
            needed, granted, ..
        } = &e
        else {
            panic!("{e:?}");
        };
        assert_eq!(needed, &["read:project", "project", "read:org"]);
        assert_eq!(granted, &["repo"]);
        assert!(e.to_string().contains("-s project,read:org."), "{e}");
    }

    #[test]
    fn a_scope_error_next_to_other_errors_keeps_the_other_messages() {
        let mut v: Value = serde_json::from_str(&scope_errors_body(3)).unwrap();
        let errs = v["errors"].as_array_mut().unwrap();
        errs.push(json!({"message": "Could not resolve"}));
        errs.push(json!({"message": "Could not resolve"}));
        let e = parse_graphql(200, &v.to_string()).unwrap_err();
        let GhError::InsufficientScopes { other, .. } = &e else {
            panic!("{e:?}");
        };
        assert_eq!(other, &["Could not resolve"]);
        let text = e.to_string();
        assert!(text.contains("lacks the scope read:project"), "{text}");
        assert!(
            text.ends_with("Other GitHub GraphQL errors: Could not resolve"),
            "{text}"
        );
    }

    #[test]
    fn an_insufficient_scopes_error_without_a_parsable_message_stays_a_plain_error() {
        let body = r#"{"errors":[{"type":"INSUFFICIENT_SCOPES","message":"odd"}]}"#;
        assert_eq!(
            parse_graphql(200, body).unwrap_err(),
            GhError::GraphQl("odd".into())
        );
    }

    #[test]
    fn other_graphql_errors_are_deduplicated_in_order_and_capped() {
        let msgs = ["a", "b", "a", "c", "b", "d", "e", "f", "a"];
        let errs: Vec<Value> = msgs.iter().map(|m| json!({"message": m})).collect();
        let body = json!({"errors": errs}).to_string();
        assert_eq!(
            parse_graphql(200, &body).unwrap_err(),
            GhError::GraphQl("a; b; c; and 3 more".into())
        );
        let body = json!({"errors": [{"message": "a"}, {"message": "a"}]}).to_string();
        assert_eq!(
            parse_graphql(200, &body).unwrap_err(),
            GhError::GraphQl("a".into())
        );
    }

    #[test]
    fn graphql_200_with_data_passes_and_warnings_are_not_errors() {
        let body = r#"{"data":{"viewer":{"login":"x"}},"extensions":{"warnings":[{"message":"deprecated"}]}}"#;
        let d = parse_graphql(200, body).unwrap();
        assert_eq!(d["viewer"]["login"], "x");
        // an empty errors array is no failure either
        assert!(parse_graphql(200, r#"{"data":{"a":1},"errors":[]}"#).is_ok());
    }

    #[test]
    fn graphql_bad_shapes_and_statuses_are_errors() {
        assert!(matches!(
            parse_graphql(200, "<html>bad gateway</html>"),
            Err(GhError::Shape(_))
        ));
        assert!(matches!(parse_graphql(200, "[]"), Err(GhError::Shape(_))));
        assert!(matches!(
            parse_graphql(200, r#"{"data":null}"#),
            Err(GhError::Shape(_))
        ));
        assert!(matches!(
            parse_graphql(502, "bad gateway"),
            Err(GhError::Http { status: 502, .. })
        ));
        assert!(matches!(parse_graphql(401, "{}"), Err(GhError::Auth(_))));
    }

    #[test]
    fn conflict_is_retried_three_times_then_fails() {
        let calls = Cell::new(0);
        let r: Result<(), _> = retry_conflict(
            3,
            |_| {},
            || {
                calls.set(calls.get() + 1);
                Err(GhError::GraphQl(
                    "Your attempt created a temporary conflict.".into(),
                ))
            },
        );
        assert!(matches!(r, Err(GhError::ConflictExhausted(_))));
        assert_eq!(calls.get(), 4, "1 try + 3 retries");
    }

    #[test]
    fn conflict_then_success_returns_the_success() {
        let calls = Cell::new(0);
        let waits = Cell::new(0u128);
        let r = retry_conflict(
            3,
            |d| waits.set(waits.get() + d.as_millis()),
            || {
                calls.set(calls.get() + 1);
                if calls.get() < 3 {
                    Err(GhError::GraphQl("temporary conflict".into()))
                } else {
                    Ok(7)
                }
            },
        );
        assert_eq!(r, Ok(7));
        assert_eq!(calls.get(), 3);
        assert_eq!(waits.get(), 500 + 1000, "backoff doubles");
    }

    #[test]
    fn other_errors_are_not_retried() {
        let calls = Cell::new(0);
        let r: Result<(), _> = retry_conflict(
            3,
            |_| {},
            || {
                calls.set(calls.get() + 1);
                Err(GhError::GraphQl("Could not resolve to a node".into()))
            },
        );
        assert!(matches!(r, Err(GhError::GraphQl(_))));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn only_an_already_exists_422_means_reuse() {
        let dup = GhError::Http {
            status: 422,
            message:
                r#"Validation Failed [{"resource":"Label","code":"already_exists","field":"name"}]"#
                    .into(),
            accepted_permissions: None,
        };
        assert!(is_already_exists(&dup));
        let other = GhError::Http {
            status: 422,
            message: r#"Validation Failed [{"resource":"Label","code":"invalid","field":"color"}]"#
                .into(),
            accepted_permissions: None,
        };
        assert!(!is_already_exists(&other));
        assert!(!is_already_exists(&GhError::Http {
            status: 404,
            message: "already_exists".into(),
            accepted_permissions: None,
        }));
    }

    #[test]
    fn the_viewer_login_is_read_from_the_user_answer() {
        assert_eq!(
            parse_viewer_login(&json!({"login": "crew-bot", "id": 1})).unwrap(),
            "crew-bot"
        );
        for bad in [
            json!({}),
            json!({"login": ""}),
            json!({"login": 3}),
            json!([]),
        ] {
            assert!(
                matches!(parse_viewer_login(&bad), Err(GhError::Shape(_))),
                "{bad}"
            );
        }
    }

    fn net() -> GhError {
        GhError::Network("error sending request".into())
    }

    #[test]
    fn a_transient_error_then_success_is_one_success_after_two_attempts() {
        let calls = Cell::new(0);
        let waits = Cell::new(0u64);
        let r = retry_transient(
            3,
            |d| waits.set(waits.get() + d.as_secs()),
            || {
                calls.set(calls.get() + 1);
                if calls.get() < 2 { Err(net()) } else { Ok(5) }
            },
        );
        assert_eq!(r, Ok(5));
        assert_eq!(calls.get(), 2);
        assert_eq!(waits.get(), 2);
    }

    #[test]
    fn a_persistent_transient_error_stays_a_network_error_and_names_the_attempts() {
        let calls = Cell::new(0);
        let waits = Cell::new(0u64);
        let r: Result<(), _> = retry_transient(
            3,
            |d| waits.set(waits.get() + d.as_secs()),
            || {
                calls.set(calls.get() + 1);
                Err(net())
            },
        );
        let e = r.unwrap_err();
        assert!(
            matches!(&e, GhError::Network(m) if m.contains("after 3 attempts")),
            "{e}"
        );
        assert_eq!(calls.get(), 3);
        assert_eq!(waits.get(), 7);
        // the exit code stays 3
        assert_eq!(
            crate::error::CbError::from(e).exit(),
            crate::exit::Exit::Network
        );
    }

    #[test]
    fn only_transport_errors_and_502_503_504_are_transient() {
        let http = |s| GhError::Http {
            status: s,
            message: "m".into(),
            accepted_permissions: None,
        };
        for s in [502, 503, 504] {
            assert!(is_transient(&http(s)), "{s}");
        }
        for s in [400, 403, 404, 422, 500] {
            assert!(!is_transient(&http(s)), "{s}");
        }
        assert!(is_transient(&net()));
        for e in [
            GhError::Auth("x".into()),
            GhError::GraphQl("x".into()),
            GhError::Shape("x".into()),
        ] {
            assert!(!is_transient(&e));
        }
        // a non-transient error is returned at once, after one call
        let calls = Cell::new(0);
        let r: Result<(), _> = retry_transient(
            3,
            |_| {},
            || {
                calls.set(calls.get() + 1);
                Err(http(404))
            },
        );
        assert_eq!(calls.get(), 1);
        assert!(r.is_err());
    }

    #[test]
    fn queries_and_field_value_updates_repeat_but_other_mutations_do_not() {
        assert!(graphql_is_repeatable("query{viewer{login}}"));
        assert!(graphql_is_repeatable(
            "mutation($p:ID!){updateProjectV2ItemFieldValue(input:{projectId:$p}){projectV2Item{id}}}"
        ));
        assert!(graphql_is_repeatable(
            "mutation($d:ID!){updateProjectV2DraftIssue(input:{draftIssueId:$d}){draftIssue{id}}}"
        ));
        assert!(graphql_is_repeatable(
            "mutation($p:ID!){updateProjectV2Collaborators(input:{projectId:$p}){collaborators{totalCount}}}"
        ));
        for m in [
            "mutation($p:ID!){addProjectV2DraftIssue(input:{projectId:$p}){projectItem{id}}}",
            "mutation($o:ID!){createProjectV2(input:{ownerId:$o}){projectV2{number}}}",
            "mutation($p:ID!){deleteProjectV2Item(input:{projectId:$p}){deletedItemId}}",
            "mutation{somethingNew(input:{a:1}){x}}",
            "mutation{noInputHere}",
        ] {
            assert!(!graphql_is_repeatable(m), "{m}");
        }
    }

    fn client_for(base: &str) -> Client {
        Client::with_base(Token::new("secret-token-xyz"), base.to_string()).unwrap()
    }

    #[test]
    fn url_follows_relative_paths_and_absolute_urls_under_the_base() {
        let c = client_for("https://api.github.com");
        assert_eq!(c.url("user").unwrap(), "https://api.github.com/user");
        assert_eq!(c.url("/user").unwrap(), "https://api.github.com/user");
        assert_eq!(
            c.url("https://api.github.com/x?page=2").unwrap(),
            "https://api.github.com/x?page=2"
        );
        let l = client_for("http://127.0.0.1:4321");
        assert_eq!(
            l.url("http://127.0.0.1:4321/x").unwrap(),
            "http://127.0.0.1:4321/x"
        );
    }

    #[test]
    fn url_refuses_an_absolute_url_outside_the_base_and_names_only_the_host() {
        let c = client_for("https://api.github.com");
        for (bad, host) in [
            ("https://evil.example/x", "evil.example"),
            (
                "https://api.github.com.evil.example/x",
                "api.github.com.evil.example",
            ),
            ("https://api.github.com@evil.example/x", "evil.example"),
            ("HTTPS://evil.example/x", "evil.example"),
            ("http://api.github.com/x", "api.github.com"),
            ("https://api.github.com", "api.github.com"),
        ] {
            let e = c.url(bad).unwrap_err().to_string();
            assert!(e.contains(host), "{bad}: {e}");
            assert!(e.contains("no request was sent"), "{bad}: {e}");
            assert!(!e.contains("secret-token-xyz"), "{e}");
        }
        let l = client_for("http://127.0.0.1:4321");
        let e = l.url("https://api.github.com/x").unwrap_err().to_string();
        assert!(e.contains("api.github.com"), "{e}");
    }

    /// A one-thread HTTP server on loopback that answers `[]` and counts connections.
    fn counting_server() -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let n = std::sync::Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        std::thread::spawn(move || {
            for conn in l.incoming() {
                let Ok(mut c) = conn else { continue };
                n2.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 4096];
                let _ = c.read(&mut buf);
                let _ = c.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n[]",
                );
            }
        });
        (port, n)
    }

    #[test]
    fn get_follows_a_loopback_url_and_sends_no_request_for_a_foreign_one() {
        use std::sync::atomic::Ordering;
        let (port, hits) = counting_server();
        let c = client_for(&format!("http://127.0.0.1:{port}"));
        // Pass direction: an absolute URL under the base is followed.
        let r = c.get(&format!("http://127.0.0.1:{port}/x")).unwrap();
        assert_eq!(r.body, json!([]));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        // Refusal direction: other absolute URLs fail and the server sees no new connection.
        for bad in [
            "https://api.github.com/x".to_string(),
            format!("https://127.0.0.1:{port}/x"),
            format!("http://127.0.0.1:{}/x", port.wrapping_add(1)),
        ] {
            let e = c.get(&bad).unwrap_err();
            assert!(matches!(e, GhError::Shape(_)), "{bad}: {e}");
            assert!(!e.to_string().contains("secret-token-xyz"));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn api_base_accepts_loopback_http_only() {
        for ok in [
            "http://127.0.0.1:8080",
            "http://127.0.0.1",
            "http://localhost:3000/",
            "http://[::1]:9",
        ] {
            let b = parse_api_base(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
            assert!(!b.ends_with('/'), "{b}");
        }
    }

    #[test]
    fn api_base_refuses_foreign_hosts_and_tricks() {
        for bad in [
            "https://evil.example",
            "https://127.0.0.1:8080",
            "http://10.0.0.1",
            "http://localhost.evil.example",
            "http://localhost.evil.example:80",
            "http://127.0.0.1.evil.example",
            "http://evil.example/127.0.0.1",
            "http://localhost@evil.example",
            "http://127.0.0.1@evil.example",
            "http://evil.example#@localhost",
            "http://localhost:80x",
            "http://127.0.0.1:8080/path",
            "http://[::1",
            "ftp://localhost",
            "localhost:8080",
            "",
        ] {
            let e = parse_api_base(bad).unwrap_err();
            assert!(matches!(e, GhError::Auth(_)), "{bad}");
            assert!(e.to_string().contains(API_BASE_ENV), "{e}");
        }
    }

    #[test]
    fn token_is_redacted_in_debug() {
        let t = Token::new("ghp_secretvalue");
        assert!(!format!("{t:?}").contains("secretvalue"));
        let c = Client::new(t).unwrap();
        assert!(!format!("{c:?}").contains("secretvalue"));
    }

    #[test]
    fn link_header_next_is_found() {
        let mut h = HeaderMap::new();
        h.insert(
            "link",
            r#"<https://api.github.com/x?page=2>; rel="next", <https://api.github.com/x?page=5>; rel="last""#
                .parse()
                .unwrap(),
        );
        assert_eq!(
            next_link(&h).as_deref(),
            Some("https://api.github.com/x?page=2")
        );
        let mut h2 = HeaderMap::new();
        h2.insert(
            "link",
            r#"<https://api.github.com/x?page=1>; rel="prev""#.parse().unwrap(),
        );
        assert_eq!(next_link(&h2), None);
    }
}
