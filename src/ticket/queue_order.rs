//! The queue order of an agent, from the rows of the board: which tickets count, and the `Order`
//! of a new ticket at the end of a queue.
//!
//! The queue of an agent is its open work: the tickets with its `Agent` in Ready, In Progress or
//! Blocked. A ticket in Review waits for the coordinator and is no more in the queue. `Order` is
//! the position, lowest first.
//!
//! Pure: callers read the rows and pass them in.

use super::TicketRow;
use crate::rules::status::Status;

/// The row is open work of an agent.
fn is_open_work(r: &TicketRow) -> bool {
    r.agent.is_some()
        && matches!(
            r.status_exact(),
            Some(Status::Ready | Status::InProgress | Status::Blocked)
        )
}

/// The open work of `agent`.
fn open_of<'a>(rows: &'a [TicketRow], agent: &'a str) -> impl Iterator<Item = &'a TicketRow> {
    rows.iter()
        .filter(move |r| is_open_work(r) && r.is_agent(agent))
}

/// The `Order` that puts a new ticket at the end of the queue of `agent`: one above the highest
/// `Order` of its open tickets, or 1 for an empty queue.
pub fn append_order(rows: &[TicketRow], agent: &str) -> i64 {
    open_of(rows, agent)
        .filter_map(|r| r.order)
        .max()
        .map_or(1, |m| m.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(n: u64, status: &str, agent: Option<&str>, order: Option<i64>) -> TicketRow {
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

    #[test]
    fn a_new_ticket_goes_after_the_highest_order_of_the_open_tickets_of_that_agent() {
        let rows = vec![
            row(1, "Ready", Some("a@x"), Some(3)),
            // the running ticket counts: the new ticket must not jump ahead of it
            row(2, "In Progress", Some("a@x"), Some(7)),
            row(3, "Blocked", Some("a@x"), Some(5)),
            row(4, "Ready", Some("b@x"), Some(99)),
            row(5, "Done", Some("a@x"), Some(500)),
        ];
        assert_eq!(append_order(&rows, "a@x"), 8);
        assert_eq!(append_order(&rows, "b@x"), 100);
        // control: an empty queue starts at 1
        assert_eq!(append_order(&rows, "c@x"), 1);
        // a negative front order (a requeue) still gets a value above the highest
        assert_eq!(
            append_order(&[row(1, "Ready", Some("a@x"), Some(-1))], "a@x"),
            0
        );
    }
}
