//! Builders for the unit tests: a board row and a verified message, with the values that most
//! cases do not care about. A case that needs another value sets it with struct update syntax,
//! for example `TicketRow { repo: "o/r".into(), ..row(1, "Ready", None, None) }`.

use crate::protocol::names::{Kind, Re};
use crate::rules::Role;
use crate::ticket::{Sender, TicketRow, VMsg};

/// The board row of `a/b#<n>` (item `PVTI_<n>`) with `status`, `agent` and `order`.
pub(crate) fn row(n: u64, status: &str, agent: Option<&str>, order: Option<i64>) -> TicketRow {
    TicketRow {
        repo: "a/b".into(),
        issue: n,
        item_id: format!("PVTI_{n}"),
        status: Some(status.into()),
        agent: agent.map(str::to_string),
        order,
        updated: None,
    }
}

/// The sender of the agent `name` in `role`.
pub(crate) fn agent(name: &str, role: Role) -> Sender {
    Sender::Agent {
        name: name.into(),
        role,
    }
}

/// A verified message: comment `id`, created at 0, without `ts` and `re`. `kind` is `None` for
/// an owner comment.
pub(crate) fn msg(id: u64, sender: Sender, to: &str, kind: Option<Kind>, body: &str) -> VMsg {
    VMsg {
        comment_id: id,
        created: 0,
        ts: String::new(),
        sender,
        to: to.into(),
        kind,
        re: Re::None,
        body: body.into(),
    }
}
