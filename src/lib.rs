//! crewboard: a GitHub Project board as the channel for a crew of AI agents.
//!
//! The crate has one binary (`src/main.rs`). The modules live in a library target so that
//! the integration tests in `tests/` can reach them. Only `cli` and `exit` (used by `main.rs`)
//! and the modules that `tests/` import are `pub`. The test-only ones carry `#[doc(hidden)]`:
//! they are not a supported API. A module that neither `main.rs` nor `tests/` needs is
//! `pub(crate)`, so rustc reports dead code in it. `testkit` holds the builders of the unit tests
//! and exists only in test builds. `tests/layering.rs` freezes the import graph between the
//! modules.

// The test code of the crate: a test fails by panic and may wait for real processes.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::let_underscore_must_use,
        clippy::disallowed_methods,
    )
)]
// A host without Unix process control builds the wake and lock engines but has no command that
// reaches them (`wake`, `lock` and `agent add` refuse there), so their items look unused. Dead
// code stays a hard error on Unix, where every item has a caller. Other lints stay on.
#![cfg_attr(not(unix), allow(dead_code))]

pub(crate) mod agent_env;
pub(crate) mod agent_gh;
pub(crate) mod audit;
pub(crate) mod authority;
pub(crate) mod base;
#[doc(hidden)]
pub mod board;
pub(crate) mod builtin_workflows;
pub(crate) mod capabilities;
pub mod cli;
#[doc(hidden)]
pub mod clock;
#[doc(hidden)]
pub mod comments;
#[doc(hidden)]
pub mod config;
pub(crate) mod ctx;
pub(crate) mod cursor;
pub(crate) mod domain;
pub(crate) mod error;
pub(crate) mod events;
pub mod exit;
#[doc(hidden)]
pub mod feed;
pub(crate) mod fsutil;
#[doc(hidden)]
pub mod github;
pub(crate) mod headless;
pub(crate) mod items;
#[doc(hidden)]
pub mod labels;
pub(crate) mod late;
pub(crate) mod lock;
#[doc(hidden)]
pub mod ops;
#[doc(hidden)]
pub mod owner_cmd;
pub(crate) mod owner_route;
pub(crate) mod profile;
#[doc(hidden)]
pub mod protocol;
#[doc(hidden)]
pub mod registry;
pub(crate) mod repeat;
#[doc(hidden)]
pub mod rules;
pub(crate) mod secrets;
pub(crate) mod services;
#[doc(hidden)]
pub mod skill_embed;
pub(crate) mod spawn;
#[doc(hidden)]
pub mod state;
#[cfg(test)]
pub(crate) mod testkit;
pub(crate) mod ticket;
pub(crate) mod toolchain;
pub(crate) mod unread;
pub(crate) mod wake;
