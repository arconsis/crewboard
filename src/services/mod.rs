//! Use cases that combine GitHub reads with the domain rules and have no command line of
//! their own: finding the tickets of a board ([`ticket_index`]), loading one ticket
//! ([`ticket_loader`]), and reading CI evidence and workflow files (`ci_evidence`,
//! `ci_triggers`). A command in `ops` calls these; they never call `ops`.

pub(crate) mod branch_guard;
pub(crate) mod ci_evidence;
pub(crate) mod ci_triggers;
pub(crate) mod ticket_index;
pub(crate) mod ticket_loader;

use crate::board::Board;
use crate::config::BoardConfig;
use crate::github::Client;

/// What a read needs: the client, the board config and the board schema. `Ctx` lends these
/// (`Ctx::reader`).
#[derive(Clone, Copy)]
pub(crate) struct Reader<'a> {
    pub client: &'a Client,
    pub cfg: &'a BoardConfig,
    pub board: &'a Board,
}
