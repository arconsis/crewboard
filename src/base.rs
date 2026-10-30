//! Base types that every module may import: who acts ([`Role`], [`Actor`]), a rule said no
//! ([`Refusal`], [`Verdict`]) and a GitHub failure ([`GhError`]). This module imports nothing
//! from the crate. [`ignore`] names the reasons for dropping a result on purpose. [`ids`] is the
//! catalog of the rule ids that a refusal names. [`fields::Field`] names one field of the board.

pub mod fields;
pub mod ids;
pub mod ignore;

use std::fmt;

/// What a tool name may look like, for the error of a bad name.
pub const TOOL_NAME_RULE: &str = "a-z, 0-9, '-', '_', '.', '+'; at most 40 characters";

/// Tool names: what a command line can start safely. No path, no space. The agent config
/// (`tools`), `join --tool` and `detect_tools` of `.crewboard.toml` use it.
pub fn valid_tool_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.' | '+')
        })
        && !s.starts_with('-')
}

/// Is `y-mo-d h:mi:se` a time that exists on the calendar (UTC)? A leap second (`se` 60) does not
/// count: the clocks of GitHub and of crewboard never show one, and `2026-02-31` must not roll
/// over to 3 March. The one check behind both timestamp parsers (`clock::parse_ts`,
/// `protocol::names::Timestamp`).
pub fn valid_civil_time(y: u32, mo: u32, d: u32, h: u32, mi: u32, se: u32) -> bool {
    let leap = (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400);
    let days = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&d) && h <= 23 && mi <= 59 && se <= 59
}

/// An agent's role on the board (`Role` field, spec/board#fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Coordinator,
    Dev,
}

impl Role {
    /// The role named `dev` or `coordinator` (the spelling of the CLI and the owner commands).
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "dev" => Some(Role::Dev),
            "coordinator" => Some(Role::Coordinator),
            _ => None,
        }
    }
}

impl Role {
    /// Like [`Role::parse`], with the text that the CLI and the owner commands print.
    pub fn parse_named(s: &str) -> Result<Role, String> {
        Role::parse(s).ok_or_else(|| format!("role '{s}' is not dev or coordinator"))
    }
}

/// Who performs an action: the owner, or an agent with a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Owner,
    Agent(Role),
}

impl Actor {
    pub fn is_owner(self) -> bool {
        self == Actor::Owner
    }
    pub fn is_coordinator(self) -> bool {
        self == Actor::Agent(Role::Coordinator)
    }
    pub fn is_dev(self) -> bool {
        self == Actor::Agent(Role::Dev)
    }
}

/// A rule said no. `rule` is an id of the catalog ([`ids`]); the CLI prints it and exits 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub rule: ids::RuleId,
    pub message: String,
}

impl Refusal {
    pub fn new(rule: ids::RuleId, message: impl Into<String>) -> Self {
        Refusal {
            rule,
            message: message.into(),
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "refused [rule={}]: {}", self.rule, self.message)
    }
}

impl std::error::Error for Refusal {}

pub type Verdict = Result<(), Refusal>;

/// Everything that can go wrong talking to GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhError {
    /// No token, or the token was refused (HTTP 401).
    Auth(String),
    /// Transport failure: DNS, TLS, timeout, connection reset.
    Network(String),
    /// HTTP status >= 400 on REST (or a non-200 GraphQL status).
    /// `accepted_permissions` is the value of the `x-accepted-github-permissions` header
    /// (e.g. `checks=read`), kept so a 403 can name the permission the token lacks.
    Http {
        status: u16,
        message: String,
        accepted_permissions: Option<String>,
    },
    /// HTTP 200 on GraphQL, but the `errors` array is present.
    GraphQl(String),
    /// HTTP 200 on GraphQL, with `INSUFFICIENT_SCOPES` errors: the token lacks a scope.
    /// `needed` is the union of the "requires one of" lists, `granted` the scopes of the token,
    /// `other` the distinct messages of any other errors in the same answer.
    InsufficientScopes {
        needed: Vec<String>,
        granted: Vec<String>,
        other: Vec<String>,
    },
    /// The answer has a different shape than expected.
    Shape(String),
    /// A list that the caller needs complete has more pages than crewboard reads
    /// (`github::paging::MAX_PAGES`). A repeat gives the same answer.
    PageLimit(String),
    /// "temporary conflict" after the retries were used up.
    ConflictExhausted(String),
}

impl fmt::Display for GhError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GhError::Auth(m) => write!(f, "GitHub authentication failed: {m}"),
            GhError::Network(m) => write!(f, "network error: {m}"),
            GhError::Http {
                status, message, ..
            } => write!(f, "GitHub HTTP {status}: {message}"),
            GhError::GraphQl(m) => write!(f, "GitHub GraphQL error: {m}"),
            GhError::InsufficientScopes {
                needed,
                granted,
                other,
            } => {
                write!(f, "{}", scope_message(needed, granted))?;
                if !other.is_empty() {
                    write!(f, " Other GitHub GraphQL errors: {}", other.join("; "))?;
                }
                Ok(())
            }
            GhError::Shape(m) => write!(f, "unexpected GitHub answer: {m}"),
            GhError::PageLimit(m) => write!(f, "list too long: {m}"),
            GhError::ConflictExhausted(m) => {
                write!(f, "GitHub write conflict persisted after 3 retries: {m}")
            }
        }
    }
}

/// The scope name for `gh auth refresh -s`: the two project scopes are one scope for `gh`.
fn refresh_scope(scope: &str) -> &str {
    match scope {
        "read:project" | "project" => "project",
        other => other,
    }
}

/// One line for a token that lacks scopes: what is missing, what the token has, the fix.
fn scope_message(needed: &[String], granted: &[String]) -> String {
    let mut refresh: Vec<&str> = Vec::new();
    for n in needed {
        let r = refresh_scope(n);
        if !refresh.contains(&r) {
            refresh.push(r);
        }
    }
    let has = if granted.is_empty() {
        "none".to_string()
    } else {
        granted.join(", ")
    };
    let (noun, pron) = if needed.len() == 1 {
        ("scope", "that scope")
    } else {
        ("scopes", "those scopes")
    };
    format!(
        "the GitHub token lacks the {noun} {} (it has: {has}). For a gh login: gh auth refresh -h github.com -s {}. Or create a token with {pron}.",
        needed.join(", "),
        refresh.join(","),
    )
}

impl std::error::Error for GhError {}
