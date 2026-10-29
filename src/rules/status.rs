//! Ticket status transitions and their writers (spec/tickets#status-flow-and-writers).
//!
//! Pure. The ticket commands in `ops/tickets*` ask it before they write the Status field.

use super::{Actor, Refusal, Verdict, ids, require};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Backlog,
    Ready,
    InProgress,
    Review,
    Blocked,
    /// A PR merged at a verified head, or merged by the owner
    /// (spec/tickets#status-flow-and-writers). Final.
    Done,
    /// Ended by the coordinator's judgment without a merged PR (`close --why`). Final.
    Dropped,
}

impl Status {
    pub const ALL: [Status; 7] = [
        Status::Backlog,
        Status::Ready,
        Status::InProgress,
        Status::Review,
        Status::Blocked,
        Status::Done,
        Status::Dropped,
    ];

    /// The option name on the board.
    pub fn name(self) -> &'static str {
        match self {
            Status::Backlog => "Backlog",
            Status::Ready => "Ready",
            Status::InProgress => "In Progress",
            Status::Review => "Review",
            Status::Blocked => "Blocked",
            Status::Done => "Done",
            Status::Dropped => "Dropped",
        }
    }

    /// Done or Dropped: the ticket has ended, and only the owner moves it out
    /// (spec/tickets#status-flow-and-writers). The readers that skip finished work ask this.
    pub fn ended(self) -> bool {
        matches!(self, Status::Done | Status::Dropped)
    }

    /// The exact option name, or `None`.
    pub fn parse(s: &str) -> Option<Status> {
        Status::ALL.into_iter().find(|x| x.name() == s)
    }

    /// True when the raw board value is exactly this status: the typed form of
    /// `value == Some("Ready")`. A legacy option (`Needs Human`) and an unknown value are not a
    /// match; use [`Status::parse_board`] to read the legacy option as Blocked.
    pub fn is(self, raw: Option<&str>) -> bool {
        raw == Some(self.name())
    }

    /// The value of the board field. A board can still hold the option `Needs Human` that an
    /// older binary wrote; it reads as Blocked, and the log says whether a question is open.
    pub fn parse_board(s: &str) -> Option<Status> {
        if s == "Needs Human" {
            return Some(Status::Blocked);
        }
        Status::parse(s)
    }
}

/// The Status field of a board item, parsed once where the board is read. The raw text stays
/// available for every value, so output that shows the board value is unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardStatus {
    /// An exact option name.
    Known(Status),
    /// The legacy option `Needs Human` that an older binary wrote. It reads as Blocked through
    /// [`BoardStatus::board`] only; the exact readers do not match it.
    NeedsHuman,
    /// Any other text, kept as it is.
    Other(String),
}

impl BoardStatus {
    pub fn parse(raw: &str) -> BoardStatus {
        match Status::parse(raw) {
            Some(s) => BoardStatus::Known(s),
            None if raw == "Needs Human" => BoardStatus::NeedsHuman,
            None => BoardStatus::Other(raw.to_string()),
        }
    }

    /// The text on the board.
    pub fn as_str(&self) -> &str {
        match self {
            BoardStatus::Known(s) => s.name(),
            BoardStatus::NeedsHuman => "Needs Human",
            BoardStatus::Other(raw) => raw,
        }
    }

    /// True when the board value is exactly this status (the legacy option is not a match).
    pub fn is(&self, status: Status) -> bool {
        *self == BoardStatus::Known(status)
    }

    /// The exact status, or `None` (same as [`Status::parse`]).
    pub fn exact(&self) -> Option<Status> {
        match self {
            BoardStatus::Known(s) => Some(*s),
            _ => None,
        }
    }

    /// The status with the legacy option read as Blocked (same as [`Status::parse_board`]).
    pub fn board(&self) -> Option<Status> {
        match self {
            BoardStatus::NeedsHuman => Some(Status::Blocked),
            other => other.exact(),
        }
    }
}

impl From<&str> for BoardStatus {
    fn from(raw: &str) -> Self {
        BoardStatus::parse(raw)
    }
}

/// `verify` is a coordinator action on a ticket in Review without an open question to the owner
/// (spec/tickets#verification-by-the-coordinator). It writes no status by itself on a pass; on
/// a fail the move back goes through [`check_status_transition`]. `question_open_since` is the
/// time of the open question, if there is one (`crate::ticket::question_open`).
pub fn check_verify(
    status: Status,
    actor: Actor,
    key: &str,
    question_open_since: Option<&str>,
) -> Verdict {
    require(
        actor.is_coordinator(),
        "verify-coordinator-only",
        "verify is a coordinator action",
    )?;
    let hint = if matches!(status, Status::InProgress | Status::Blocked) {
        ". If the owner merged it, the coordinator runs `crewboard close <n>` (the PR body must close the issue, for example `Closes #<n>`)"
    } else {
        ""
    };
    require(
        status == Status::Review,
        "verify-needs-review",
        &format!(
            "verify needs a ticket in Review, this one is {}{hint}",
            status.name()
        ),
    )?;
    check_no_open_question(key, question_open_since)
}

/// `unblock` is a coordinator action on a Blocked ticket that has an assignee
/// (spec/tickets#status-flow-and-writers). It writes no status: it posts the `kind=answer` that
/// lets the assignee `take` the ticket again.
pub fn check_unblock(status: Status, actor: Actor, has_assignee: bool) -> Verdict {
    require(
        actor.is_coordinator(),
        "unblock-coordinator-only",
        "unblock is a coordinator action",
    )?;
    require(
        status == Status::Blocked,
        "unblock-needs-blocked",
        &format!(
            "unblock needs a ticket in Blocked, this one is {}",
            status.name()
        ),
    )?;
    require(
        has_assignee,
        "unblock-no-assignee",
        "unblock needs a ticket with an assignee; use `reassign` or `assign` instead",
    )
}

/// Facts that the caller reads from the ticket and the actor, before it knows which move it
/// checks. Each one is used by a single kind of move; [`TicketFacts::plain`] and the other
/// constructors hand it to that move and drop the rest.
#[derive(Debug, Clone, Copy, Default)]
pub struct TicketFacts {
    /// The actor is the dev agent assigned to this ticket.
    pub actor_is_assignee: bool,
    /// An answer was posted after the blocking message (spec/tickets#status-flow-and-writers): an
    /// owner message or a coordinator `kind=answer` after the latest `blocked` or `question`
    /// message. The caller reads this from the ticket's comments; this module does not.
    pub answer_after_block: bool,
    /// Why the ticket is blocked, when the acting agent wrote the latest `blocked` message
    /// itself: `Parked` is a park (spec/tickets#status-flow-and-writers, `take --interrupt`)
    /// that needs no answer.
    pub own_block: BlockReason,
    /// The ticket was split by the coordinator: its body has a `Split off:` line and a signed
    /// split note of a coordinator names the same ticket (`ticket::split_off_proven`).
    pub split_off_proven: bool,
}

impl TicketFacts {
    /// A move that needs no fact besides the writer: `Ready -> In Progress` and
    /// `In Progress -> Blocked`.
    pub fn plain(&self) -> Transition {
        Transition::Plain {
            actor_is_assignee: self.actor_is_assignee,
        }
    }

    /// `Blocked -> In Progress`.
    pub fn resume(&self) -> Transition {
        Transition::Resume {
            actor_is_assignee: self.actor_is_assignee,
            answer_after_block: self.answer_after_block,
            own_block: self.own_block,
        }
    }

    /// A move to Review. `new_head` is only read by `Review -> Review`.
    pub fn deliver(&self, evidence_complete: bool, new_head: bool) -> Transition {
        Transition::Deliver {
            actor_is_assignee: self.actor_is_assignee,
            evidence_complete,
            new_head,
            split_off_proven: self.split_off_proven,
        }
    }
}

/// The move that a caller asks for, with exactly the facts that this move reads. Each variant
/// belongs to the rows of the status table named on it; [`check_status_transition`] refuses a
/// variant that does not fit the `from -> to` pair as not in the table.
#[derive(Debug, Clone, Copy)]
pub enum Transition {
    /// `None -> Backlog`: a new ticket.
    Create,
    /// `Ready -> In Progress` and `In Progress -> Blocked`: the writer is the only fact.
    Plain {
        /// The actor is the dev agent assigned to this ticket.
        actor_is_assignee: bool,
    },
    /// `Backlog -> Ready`.
    Release {
        /// The `Gate` field is set and not empty.
        gate_set: bool,
        /// The `Agent` field names an agent that is `active` and has role dev.
        assignee_active_dev: bool,
    },
    /// `Blocked -> In Progress`.
    Resume {
        /// The actor is the dev agent assigned to this ticket.
        actor_is_assignee: bool,
        /// See [`TicketFacts::answer_after_block`].
        answer_after_block: bool,
        /// See [`TicketFacts::own_block`].
        own_block: BlockReason,
    },
    /// `In Progress -> Review`, the re-delivery `Review -> Review`, and `Blocked -> Review` of a
    /// split ticket.
    Deliver {
        /// The actor is the dev agent assigned to this ticket.
        actor_is_assignee: bool,
        /// The `done` message has pr, sha and ci (spec/tickets#evidence), checked against GitHub.
        evidence_complete: bool,
        /// The `done` names a head SHA that is not the SHA of the latest `done` of the assignee
        /// (or the assignee has no `done` yet). Only `Review -> Review` reads it: a re-delivery
        /// after a branch update.
        new_head: bool,
        /// See [`TicketFacts::split_off_proven`]. Only `Blocked -> Review` reads it.
        split_off_proven: bool,
    },
    /// After a failed verify: `Review -> In Progress` and `Review -> Ready`.
    AfterVerify {
        /// Result of `verify`: `None` = not run, `Some(true)` = passed, `Some(false)` = failed.
        verify: Option<bool>,
        /// The assignee has another ticket In Progress.
        assignee_busy: bool,
    },
    /// `reassign`: any status to Ready with a new agent, or to Backlog without one.
    Reassign {
        /// The agent is dead or stopped, or the ticket is blocked too long.
        justified: bool,
    },
    /// `close --why`: any status but Done and Dropped to Dropped. The writer is the only fact.
    Drop,
    /// Every move to `Done`.
    Close {
        /// Why the move happens, as the caller knows it. Every move to `Done` by a non-owner
        /// sets a reason, so `None` there means a caller forgot. The owner's own move needs
        /// none: the owner returns before any reason is read.
        reason: Option<DoneReason>,
        /// Result of `verify`; `Review -> Done` reads it.
        verify: Option<bool>,
    },
}

impl Transition {
    fn actor_is_assignee(&self) -> bool {
        match self {
            Transition::Plain { actor_is_assignee }
            | Transition::Resume {
                actor_is_assignee, ..
            }
            | Transition::Deliver {
                actor_is_assignee, ..
            } => *actor_is_assignee,
            _ => false,
        }
    }
}

/// Why a ticket is blocked, as the latest `kind=blocked` message says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlockReason {
    /// The dev parked the ticket for a ticket of higher priority (`take --interrupt`).
    Parked { for_issue: u64 },
    /// Any other block, or no block message.
    #[default]
    Plain,
}

/// The exits to `Done`. One variant per real exit, named for what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoneReason {
    /// `close --verified`: the coordinator closes a ticket in Review after a passed verify.
    VerifiedClose,
    /// `merge`: the coordinator merged the PR after a passed verify and closes the ticket.
    VerifiedMerge,
    /// `close`: the owner merged the PR by hand and the issue is closed (the caller
    /// checked the PR and the merge actor). No verify is needed, the merge made it moot.
    OwnerMerged,
}

impl DoneReason {
    /// The coordinator may end a ticket for this reason from any status but Done and Dropped,
    /// with no verify.
    fn skips_verify(self) -> bool {
        matches!(self, Self::OwnerMerged)
    }
}

/// `take --interrupt` (spec/tickets#status-flow-and-writers): the new ticket must sit strictly
/// ahead of the ticket that is In Progress, that is have a lower `Order`. The coordinator makes a ticket urgent by putting it first
/// (`assign <n> --order <number>`). A missing `Order` is never ahead. Pure.
pub fn check_interrupt_by_order(new: Option<i64>, current: Option<i64>) -> Verdict {
    let show = |o: Option<i64>| o.map_or_else(|| "none".to_string(), |o| o.to_string());
    require(
        matches!((new, current), (Some(n), Some(c)) if n < c),
        "interrupt-not-ahead",
        &format!(
            "--interrupt needs a ticket that is strictly ahead of the ticket in progress in your queue, that is a lower Order (new: {}, in progress: {}); ask the coordinator to put it first with `assign <n> --order <number>`",
            show(new),
            show(current)
        ),
    )
}

/// The refusal for any move out of Done.
fn done_final() -> Refusal {
    Refusal::new(
        "status-done-final",
        "ticket is Done; only the owner moves it (open a new ticket, or ask the owner)",
    )
}

/// The refusal for any move out of Dropped.
pub fn dropped_final() -> Refusal {
    Refusal::new(
        "status-dropped-final",
        "ticket is Dropped; only the owner moves it (`crewboard move`; or open a new ticket)",
    )
}

/// The refusal for an act on a ticket that has ended: `status-done-final` or
/// `status-dropped-final`. `None` for a ticket that has not ended.
pub fn ended_final(status: Status) -> Option<Refusal> {
    match status {
        Status::Done => Some(done_final()),
        Status::Dropped => Some(dropped_final()),
        _ => None,
    }
}

/// `ask` (spec/tickets#status-flow-and-writers): the coordinator asks the owner about any ticket
/// that has not ended, a dev agent only about its own ticket. A question is no status change by
/// itself; [`ask_moves_to`] says when the status moves too. Pure.
pub fn check_ask(key: &str, actor: Actor, is_assignee: bool, status: Status) -> Verdict {
    if let Some(r) = ended_final(status) {
        return Err(r);
    }
    require(
        actor.is_coordinator() || (actor.is_dev() && is_assignee),
        "ask-not-assignee",
        &format!("only the assignee or the coordinator asks the owner about {key}"),
    )
}

/// The status that `ask` writes after it posted the question: Blocked for the assignee's own
/// ticket that is In Progress (the agent is free to take the next one), no write otherwise.
pub fn ask_moves_to(status: Status, actor_is_assignee: bool) -> Option<Status> {
    (status == Status::InProgress && actor_is_assignee).then_some(Status::Blocked)
}

/// `verify`, `merge` and `close --verified` need a ticket without an open question to the owner
/// (rule `verify-owner-question-open`). `open_since` is the time of that question, if one is
/// open (`crate::ticket::question_open`). Pure.
pub fn check_no_open_question(key: &str, open_since: Option<&str>) -> Verdict {
    let Some(since) = open_since else {
        return Ok(());
    };
    let n = key.rsplit('#').next().unwrap_or(key);
    Err(Refusal::new(
        "verify-owner-question-open",
        format!(
            "{key} has a question to the owner since {since}: verify after the owner answers on the ticket, or take the question back with crewboard reassign {n} --to <agent> --reason \"...\"; an answer in a chat is not signed"
        ),
    ))
}

/// The latest `done` of a ticket is on the owner route: the owner applies the patch and merges
/// the PR by hand, so the evidence of that `done` is not a delivery to verify (rule
/// `verify-owner-route`). Pure.
pub fn check_not_owner_route(key: &str, on_owner_route: bool) -> Verdict {
    let n = key.rsplit('#').next().unwrap_or(key);
    require(
        !on_owner_route,
        "verify-owner-route",
        &format!(
            "the latest done of {key} is on the owner route: the owner applies the patch and merges the PR by hand, then crewboard close {n}"
        ),
    )
}

/// The `Order` that puts a ticket at the front of a queue: one below the lowest `Order` in the
/// queue, or 0 when no ticket there has an `Order`. The result can be negative: when the
/// lowest `Order` is 0, only a value below 0 is strictly first (an equal `Order` would fall
/// back to the issue number). Pure.
pub fn front_order(lowest_in_queue: Option<i64>) -> i64 {
    lowest_in_queue.map_or(0, |o| o.saturating_sub(1))
}

/// Check one status change. `from = None` means a new ticket.
///
/// The owner can always move a ticket (spec/tickets#status-flow-and-writers); everything else
/// follows the table. `Blocked -> In Progress` is allowed for the assigned dev agent only when
/// the [`Transition::Resume`] has an answer after the block. `Review -> Review` is the
/// re-delivery of the assignee with a new head ([`Transition::Deliver`]). Not in the table, so
/// refused: any other no-change move. `Done -> anything` except the owner is `status-done-final`,
/// `Dropped -> anything` except the owner is `status-dropped-final`.
pub fn check_status_transition(
    from: Option<Status>,
    to: Status,
    actor: Actor,
    tr: &Transition,
) -> Verdict {
    use Status::*;
    let by_assignee = tr.actor_is_assignee();
    // After a branch update (for example after a merge refusal) the assignee
    // delivers the new head from Review. The Status stays Review. The same head, another
    // writer or any other status is a no-change move, as before.
    if from == Some(Review)
        && to == Review
        && actor.is_dev()
        && by_assignee
        && let Transition::Deliver {
            evidence_complete,
            new_head: true,
            ..
        } = *tr
    {
        return require(
            evidence_complete,
            "status-evidence-missing",
            "a re-delivery in Review needs complete evidence: pr, sha, ci",
        );
    }
    if from == Some(to) {
        let text = if to == Review && by_assignee && !actor.is_owner() && !actor.is_coordinator() {
            "ticket is already Review: your earlier `done` landed; wait for the coordinator's verify"
                .to_string()
        } else {
            format!("ticket is already {to:?}")
        };
        return Err(Refusal::new("status-no-change", text));
    }
    if actor.is_owner() {
        return Ok(());
    }
    // Done and Dropped are final: only the owner moves a ticket out of them.
    if let Some(r) = from.and_then(ended_final) {
        return Err(r);
    }
    // The coordinator ends a ticket from any status but Done and Dropped after the owner merged
    // the PR by hand.
    if let Transition::Close {
        reason: Some(reason),
        ..
    } = *tr
        && to == Done
        && reason.skips_verify()
        && actor.is_coordinator()
        && from.is_some()
    {
        return Ok(());
    }
    // The finished part of a split ticket goes to Review without a take. Without
    // the split proof, the old paths below apply (refused).
    if from == Some(Blocked)
        && to == Review
        && let Transition::Deliver {
            evidence_complete,
            split_off_proven: true,
            ..
        } = *tr
    {
        require(
            actor.is_dev() && by_assignee,
            "status-wrong-writer",
            "Blocked -> Review (split ticket, kind=done) is an action of the assigned dev agent",
        )?;
        return require(
            evidence_complete,
            "status-evidence-missing",
            "Review needs complete evidence: pr, sha, ci",
        );
    }
    table_move(from, to, actor, tr)
}

/// The rows of the status table: one arm per `from -> to` pair, each reading only its own facts.
fn table_move(from: Option<Status>, to: Status, actor: Actor, tr: &Transition) -> Verdict {
    use Status::*;
    let by_assignee = tr.actor_is_assignee();
    let coordinator = |what: &str| {
        require(
            actor.is_coordinator(),
            "status-wrong-writer",
            &format!("{what} is a coordinator action"),
        )
    };
    let assignee = |what: &str| {
        require(
            actor.is_dev() && by_assignee,
            "status-wrong-writer",
            &format!("{what} is an action of the assigned dev agent"),
        )
    };
    // The variant does not carry the facts of this move: the pair is not in the table.
    let not_in_table = || {
        Err(Refusal::new(
            "status-unknown-transition",
            format!("{from:?} -> {to:?} is not in the status table"),
        ))
    };
    match (from, to, *tr) {
        (None, Backlog, Transition::Create) => coordinator("creating a ticket in Backlog"),
        (
            Some(Backlog),
            Ready,
            Transition::Release {
                gate_set,
                assignee_active_dev,
            },
        ) => {
            coordinator("Backlog -> Ready")?;
            require(
                gate_set,
                ids::STATUS_GATE_MISSING,
                "Backlog -> Ready needs a Gate",
            )?;
            require(
                assignee_active_dev,
                "status-agent-not-active-dev",
                "Backlog -> Ready needs the Agent field set to an active dev agent",
            )
        }
        (Some(Ready), InProgress, Transition::Plain { .. }) => {
            assignee("Ready -> In Progress (kind=ack)")
        }
        (
            Some(InProgress),
            Review,
            Transition::Deliver {
                evidence_complete, ..
            },
        ) => {
            assignee("In Progress -> Review (kind=done)")?;
            require(
                evidence_complete,
                "status-evidence-missing",
                "Review needs complete evidence: pr, sha, ci",
            )
        }
        (Some(InProgress), Blocked, Transition::Plain { .. }) => {
            assignee("In Progress -> Blocked (kind=blocked)")
        }
        (
            Some(Blocked),
            InProgress,
            Transition::Resume {
                answer_after_block,
                own_block,
                ..
            },
        ) => {
            assignee("Blocked -> In Progress")?;
            require(
                answer_after_block || matches!(own_block, BlockReason::Parked { .. }),
                ids::STATUS_ANSWER_MISSING,
                "Blocked -> In Progress needs an owner message or a coordinator answer after the latest blocked or question message (a ticket you parked with `take --interrupt` needs none)",
            )
        }
        (Some(Review), Done, Transition::Close { .. })
        | (Some(Review), InProgress, Transition::AfterVerify { .. }) => {
            coordinator(&format!("Review -> {}", to.name()))?;
            after_verify(to, tr)
        }
        // A failed verify of a ticket whose agent works on another ticket
        (
            Some(Review),
            Ready,
            Transition::AfterVerify {
                verify: Some(false),
                ..
            },
        ) => {
            coordinator("Review -> Ready (failed verify)")?;
            after_verify(to, tr)
        }
        // `close --why`: the coordinator drops a ticket that has not ended (an ended one is
        // refused above).
        (Some(_), Dropped, Transition::Drop) => coordinator("close --why (-> Dropped)"),
        // `reassign`: to Ready with a new agent, to Backlog without one (a ticket in Ready
        // always has an agent, `rules::reassign::target_status`).
        (Some(_), Ready | Backlog, Transition::Reassign { justified }) => {
            coordinator(&format!("reassign (-> {})", to.name()))?;
            require(
                justified,
                "status-reassign-reason",
                "reassign needs a reason: dead, stopping, late or blocked-too-long",
            )
        }
        _ => not_in_table(),
    }
}

/// The move of the coordinator after a verify from Review: Done after a pass; after a fail In
/// Progress for an idle agent or Ready for a busy one.
fn after_verify(to: Status, tr: &Transition) -> Verdict {
    match (to, *tr) {
        (Status::Done, Transition::Close { verify, .. }) => require(
            verify == Some(true),
            "status-verify-required",
            "Review -> Done needs a passed verify",
        ),
        (
            Status::InProgress,
            Transition::AfterVerify {
                verify,
                assignee_busy,
            },
        ) => require(
            verify == Some(false),
            "status-verify-failed-required",
            "Review -> In Progress needs a failed verify",
        )
        .and_then(|()| {
            require(
                !assignee_busy,
                "status-verify-failed-agent-busy",
                "the agent has another ticket In Progress: a failed verify queues the ticket in Ready (Review -> Ready)",
            )
        }),
        (
            Status::Ready,
            Transition::AfterVerify {
                verify: Some(false),
                assignee_busy,
            },
        ) => require(
            assignee_busy,
            "status-verify-requeue-agent-idle",
            "the agent has no other ticket In Progress: a failed verify moves the ticket to In Progress",
        ),
        _ => Err(Refusal::new(
            "status-unknown-transition",
            format!("Review -> {to:?} is not a move after a verify"),
        )),
    }
}

#[cfg(test)]
mod table;

#[cfg(test)]
mod tests {
    use super::Status::*;
    use super::table::{Profile as TicketCtx, check};
    use super::*;
    use crate::rules::Role;

    /// The tests write the facts of a move as one flat record (`TicketCtx`); `check` picks the
    /// typed move and hands it the facts that this move reads.
    fn check_status_transition(
        from: Option<Status>,
        to: Status,
        actor: Actor,
        ctx: &TicketCtx,
    ) -> Verdict {
        check(ctx, from, to, actor)
    }

    #[test]
    fn a_board_status_keeps_the_raw_text_and_reads_as_before() {
        for raw in [
            "Ready",
            "In Progress",
            "Needs Human",
            "ready",
            "Shipped",
            "",
        ] {
            let b = BoardStatus::parse(raw);
            assert_eq!(b.as_str(), raw, "the text on the board stays");
            assert_eq!(b.exact(), Status::parse(raw), "{raw}");
            assert_eq!(b.board(), Status::parse_board(raw), "{raw}");
            for st in Status::ALL {
                assert_eq!(b.is(st), st.is(Some(raw)), "{raw}");
            }
        }
        assert_eq!(BoardStatus::parse("Needs Human"), BoardStatus::NeedsHuman);
        assert_eq!(
            BoardStatus::parse("Shipped"),
            BoardStatus::Other("Shipped".into())
        );
    }

    const OWNER: Actor = Actor::Owner;
    const COORD: Actor = Actor::Agent(Role::Coordinator);
    const DEV: Actor = Actor::Agent(Role::Dev);

    fn all_ok() -> TicketCtx {
        TicketCtx {
            gate_set: true,
            assignee_active_dev: true,
            actor_is_assignee: true,
            evidence_complete: true,
            verify: Some(true),
            reassign_justified: true,
            answer_after_block: true,
            own_block: BlockReason::Plain,
            split_off_proven: false,
            assignee_busy: false,
            // only a re-delivery in Review reads it; its test sets it
            new_head: false,
            // only `close` reads it; its test sets it
            done_reason: None,
        }
    }

    fn rule(from: Option<Status>, to: Status, actor: Actor, ctx: &TicketCtx) -> &'static str {
        check_status_transition(from, to, actor, ctx)
            .expect_err("must be refused")
            .rule
    }

    #[test]
    fn a_failed_verify_moves_to_in_progress_for_an_idle_agent_and_to_ready_for_a_busy_one() {
        let failed = TicketCtx {
            verify: Some(false),
            ..all_ok()
        };
        let busy = TicketCtx {
            assignee_busy: true,
            ..failed
        };
        // idle agent: In Progress passes, Ready is refused
        assert!(check_status_transition(Some(Review), InProgress, COORD, &failed).is_ok());
        assert_eq!(
            rule(Some(Review), Ready, COORD, &failed),
            "status-verify-requeue-agent-idle"
        );
        // busy agent: Ready passes, In Progress is refused
        assert!(check_status_transition(Some(Review), Ready, COORD, &busy).is_ok());
        assert_eq!(
            rule(Some(Review), InProgress, COORD, &busy),
            "status-verify-failed-agent-busy"
        );
        // only the coordinator, and only after a failed verify (a plain reassign keeps its rule)
        assert_eq!(rule(Some(Review), Ready, DEV, &busy), "status-wrong-writer");
        let reassign = TicketCtx {
            verify: None,
            reassign_justified: false,
            ..busy
        };
        assert_eq!(
            rule(Some(Review), Ready, COORD, &reassign),
            "status-reassign-reason"
        );
    }

    #[test]
    fn the_legacy_option_reads_as_blocked_on_the_board_only() {
        assert_eq!(Status::parse_board("Needs Human"), Some(Blocked));
        assert_eq!(Status::parse("Needs Human"), None);
        for s in Status::ALL {
            assert_eq!(Status::parse_board(s.name()), Some(s));
        }
        assert_eq!(Status::parse_board("needs human"), None);
    }

    #[test]
    fn is_matches_the_exact_option_name_only() {
        for s in Status::ALL {
            assert!(s.is(Some(s.name())));
            assert!(!s.is(None));
            for other in Status::ALL.into_iter().filter(|o| *o != s) {
                assert!(!s.is(Some(other.name())));
            }
        }
        // the legacy option and an unknown value are no match, as `== Some("Blocked")` was
        assert!(!Blocked.is(Some("Needs Human")));
        assert!(!Ready.is(Some("ready")));
    }

    #[test]
    fn status_names_round_trip_and_unknown_names_are_none() {
        for s in Status::ALL {
            assert_eq!(Status::parse(s.name()), Some(s));
        }
        assert_eq!(Status::parse("in progress"), None);
        assert_eq!(Status::parse(""), None);
    }

    const KEY: &str = "o/r#12";

    fn verify_rule(s: Status, actor: Actor) -> &'static str {
        check_verify(s, actor, KEY, None)
            .expect_err("must be refused")
            .rule
    }

    #[test]
    fn verify_is_for_the_coordinator_on_a_ticket_in_review() {
        assert!(check_verify(Review, COORD, KEY, None).is_ok());
        assert_eq!(verify_rule(Review, DEV), "verify-coordinator-only");
        assert_eq!(verify_rule(Review, OWNER), "verify-coordinator-only");
        for s in [Backlog, Ready, InProgress, Blocked, Done] {
            assert_eq!(verify_rule(s, COORD), "verify-needs-review", "{s:?}");
        }
    }

    #[test]
    fn verify_in_progress_or_blocked_names_the_owner_merge_path() {
        for s in [InProgress, Blocked] {
            let m = check_verify(s, COORD, KEY, None)
                .expect_err("must be refused")
                .message;
            assert!(m.contains("crewboard close <n>"), "{s:?}: {m}");
            assert!(m.contains("Closes #<n>"), "{s:?}: {m}");
        }
        // control: the other statuses keep the short text
        let m = check_verify(Done, COORD, KEY, None)
            .expect_err("must be refused")
            .message;
        assert!(!m.contains("crewboard close"), "{m}");
    }

    #[test]
    fn new_ticket_to_backlog() {
        assert!(check_status_transition(None, Backlog, COORD, &all_ok()).is_ok());
        assert!(check_status_transition(None, Backlog, OWNER, &all_ok()).is_ok());
        assert_eq!(rule(None, Backlog, DEV, &all_ok()), "status-wrong-writer");
        assert_eq!(
            rule(None, Ready, COORD, &all_ok()),
            "status-unknown-transition"
        );
    }

    #[test]
    fn backlog_to_ready_needs_gate_and_active_dev() {
        assert!(check_status_transition(Some(Backlog), Ready, COORD, &all_ok()).is_ok());
        let no_gate = TicketCtx {
            gate_set: false,
            ..all_ok()
        };
        assert_eq!(
            rule(Some(Backlog), Ready, COORD, &no_gate),
            "status-gate-missing"
        );
        let no_dev = TicketCtx {
            assignee_active_dev: false,
            ..all_ok()
        };
        assert_eq!(
            rule(Some(Backlog), Ready, COORD, &no_dev),
            "status-agent-not-active-dev"
        );
        assert_eq!(
            rule(Some(Backlog), Ready, DEV, &all_ok()),
            "status-wrong-writer"
        );
    }

    #[test]
    fn ready_to_in_progress_only_by_assignee() {
        assert!(check_status_transition(Some(Ready), InProgress, DEV, &all_ok()).is_ok());
        let other_dev = TicketCtx {
            actor_is_assignee: false,
            ..all_ok()
        };
        assert_eq!(
            rule(Some(Ready), InProgress, DEV, &other_dev),
            "status-wrong-writer"
        );
        assert_eq!(
            rule(Some(Ready), InProgress, COORD, &all_ok()),
            "status-wrong-writer"
        );
    }

    #[test]
    fn in_progress_to_review_needs_evidence() {
        assert!(check_status_transition(Some(InProgress), Review, DEV, &all_ok()).is_ok());
        let none = TicketCtx {
            evidence_complete: false,
            ..all_ok()
        };
        assert_eq!(
            rule(Some(InProgress), Review, DEV, &none),
            "status-evidence-missing"
        );
        assert_eq!(
            rule(Some(InProgress), Review, COORD, &all_ok()),
            "status-wrong-writer"
        );
    }

    #[test]
    fn a_new_head_of_the_assignee_is_a_re_delivery_in_review_and_all_else_stays_no_change() {
        // pass: the assignee delivers a new head with complete evidence
        let new = TicketCtx {
            new_head: true,
            ..all_ok()
        };
        assert!(check_status_transition(Some(Review), Review, DEV, &new).is_ok());
        // refusals: incomplete evidence names it
        let none = TicketCtx {
            evidence_complete: false,
            ..new
        };
        assert_eq!(
            rule(Some(Review), Review, DEV, &none),
            "status-evidence-missing"
        );
        // the same head, another dev agent, the coordinator: no change, as before
        assert_eq!(
            rule(Some(Review), Review, DEV, &all_ok()),
            "status-no-change"
        );
        let other = TicketCtx {
            actor_is_assignee: false,
            ..new
        };
        assert_eq!(rule(Some(Review), Review, DEV, &other), "status-no-change");
        assert_eq!(rule(Some(Review), Review, COORD, &new), "status-no-change");
        // a new head opens no other no-change move
        for s in [Backlog, Ready, InProgress, Blocked, Done] {
            assert_eq!(rule(Some(s), s, DEV, &new), "status-no-change", "{s:?}");
        }
    }

    #[test]
    fn in_progress_to_blocked_only_by_assignee() {
        assert!(check_status_transition(Some(InProgress), Blocked, DEV, &all_ok()).is_ok());
        let other = TicketCtx {
            actor_is_assignee: false,
            ..all_ok()
        };
        assert_eq!(
            rule(Some(InProgress), Blocked, DEV, &other),
            "status-wrong-writer"
        );
    }

    #[test]
    fn review_to_done_needs_passed_verify_by_coordinator() {
        assert!(check_status_transition(Some(Review), Done, COORD, &all_ok()).is_ok());
        for v in [None, Some(false)] {
            let c = TicketCtx {
                verify: v,
                ..all_ok()
            };
            assert_eq!(
                rule(Some(Review), Done, COORD, &c),
                "status-verify-required"
            );
        }
        assert_eq!(
            rule(Some(Review), Done, DEV, &all_ok()),
            "status-wrong-writer"
        );
    }

    #[test]
    fn review_back_to_in_progress_needs_failed_verify() {
        let failed = TicketCtx {
            verify: Some(false),
            ..all_ok()
        };
        assert!(check_status_transition(Some(Review), InProgress, COORD, &failed).is_ok());
        for v in [None, Some(true)] {
            let c = TicketCtx {
                verify: v,
                ..all_ok()
            };
            assert_eq!(
                rule(Some(Review), InProgress, COORD, &c),
                "status-verify-failed-required"
            );
        }
        assert_eq!(
            rule(Some(Review), InProgress, DEV, &failed),
            "status-wrong-writer"
        );
    }

    #[test]
    fn the_front_of_a_queue_is_one_below_the_lowest_order() {
        assert_eq!(front_order(None), 0);
        assert_eq!(front_order(Some(5)), 4);
        assert_eq!(front_order(Some(1)), 0);
        // an equal Order would lose the tie-break to the issue number, so the front goes below 0
        assert_eq!(front_order(Some(0)), -1);
        assert_eq!(front_order(Some(-1)), -2);
    }

    fn no_answer() -> TicketCtx {
        TicketCtx {
            answer_after_block: false,
            ..all_ok()
        }
    }

    fn not_assignee() -> TicketCtx {
        TicketCtx {
            actor_is_assignee: false,
            ..all_ok()
        }
    }

    #[test]
    fn blocked_to_in_progress_allowed_for_assignee_with_answer() {
        assert!(check_status_transition(Some(Blocked), InProgress, DEV, &all_ok()).is_ok());
    }

    #[test]
    fn unblock_is_for_the_coordinator_on_a_blocked_ticket_with_an_assignee() {
        let c = Actor::Agent(Role::Coordinator);
        assert!(check_unblock(Blocked, c, true).is_ok());
        // refusal: each condition alone
        assert_eq!(
            check_unblock(Blocked, DEV, true).unwrap_err().rule,
            "unblock-coordinator-only"
        );
        for s in [Backlog, Ready, InProgress, Review, Done] {
            assert_eq!(
                check_unblock(s, c, true).unwrap_err().rule,
                "unblock-needs-blocked",
                "{s:?}"
            );
        }
        assert_eq!(
            check_unblock(Blocked, c, false).unwrap_err().rule,
            "unblock-no-assignee"
        );
    }

    #[test]
    fn blocked_to_in_progress_refused_without_answer() {
        assert_eq!(
            rule(Some(Blocked), InProgress, DEV, &no_answer()),
            "status-answer-missing"
        );
    }

    #[test]
    fn blocked_to_in_progress_refused_for_other_dev_and_coordinator() {
        assert_eq!(
            rule(Some(Blocked), InProgress, DEV, &not_assignee()),
            "status-wrong-writer"
        );
        assert_eq!(
            rule(Some(Blocked), InProgress, COORD, &all_ok()),
            "status-wrong-writer"
        );
    }

    #[test]
    fn reassign_to_ready_or_backlog_needs_coordinator_and_reason() {
        // Ready is the target with a new agent, Backlog without one
        for to in [Ready, Backlog] {
            for from in [InProgress, Review, Blocked] {
                assert!(
                    check_status_transition(Some(from), to, COORD, &all_ok()).is_ok(),
                    "{from:?} -> {to:?}"
                );
                let no_reason = TicketCtx {
                    reassign_justified: false,
                    ..all_ok()
                };
                assert_eq!(
                    rule(Some(from), to, COORD, &no_reason),
                    "status-reassign-reason"
                );
                assert_eq!(rule(Some(from), to, DEV, &all_ok()), "status-wrong-writer");
            }
        }
        // a Ready ticket goes to Backlog too; Backlog to Backlog and Done to Backlog are refused
        assert!(check_status_transition(Some(Ready), Backlog, COORD, &all_ok()).is_ok());
        assert_eq!(
            rule(Some(Backlog), Backlog, COORD, &all_ok()),
            "status-no-change"
        );
        assert_eq!(
            rule(Some(Done), Backlog, COORD, &all_ok()),
            "status-done-final"
        );
    }

    #[test]
    fn owner_can_always_move_but_not_to_the_same_status() {
        let empty = TicketCtx::default();
        for from in Status::ALL {
            for to in Status::ALL {
                let r = check_status_transition(Some(from), to, OWNER, &empty);
                if from == to {
                    assert_eq!(r.unwrap_err().rule, "status-no-change");
                } else {
                    assert!(r.is_ok(), "{from:?} -> {to:?}");
                }
            }
        }
    }

    #[test]
    fn transitions_outside_the_table_are_refused() {
        assert_eq!(
            rule(Some(Backlog), Done, COORD, &all_ok()),
            "status-unknown-transition"
        );
        assert_eq!(
            rule(Some(Ready), Review, DEV, &all_ok()),
            "status-unknown-transition"
        );
        assert_eq!(
            rule(Some(Done), Review, COORD, &all_ok()),
            "status-done-final"
        );
    }

    #[test]
    fn a_park_by_the_assignee_needs_no_answer_and_a_normal_block_still_does() {
        let parked = TicketCtx {
            own_block: BlockReason::Parked { for_issue: 9 },
            ..no_answer()
        };
        assert!(check_status_transition(Some(Blocked), InProgress, DEV, &parked).is_ok());
        assert_eq!(
            rule(Some(Blocked), InProgress, DEV, &no_answer()),
            "status-answer-missing"
        );
        // the park does not let another agent take the ticket back
        let other = TicketCtx {
            actor_is_assignee: false,
            ..parked
        };
        assert_eq!(
            rule(Some(Blocked), InProgress, DEV, &other),
            "status-wrong-writer"
        );
    }

    #[test]
    fn interrupt_by_order_needs_a_strictly_lower_order() {
        assert!(check_interrupt_by_order(Some(10), Some(20)).is_ok());
        assert!(check_interrupt_by_order(Some(-1), Some(0)).is_ok());
        for (new, cur) in [
            (Some(20), Some(10)),
            (Some(10), Some(10)),
            (None, Some(10)),
            (Some(10), None),
            (None, None),
        ] {
            let e = check_interrupt_by_order(new, cur).unwrap_err();
            assert_eq!(e.rule, "interrupt-not-ahead", "{new:?} over {cur:?}");
            assert!(e.message.contains("assign <n> --order"), "{}", e.message);
        }
    }

    fn split_ctx() -> TicketCtx {
        TicketCtx {
            split_off_proven: true,
            answer_after_block: false,
            ..all_ok()
        }
    }

    #[test]
    fn a_split_ticket_goes_from_blocked_to_review_by_its_assignee_only() {
        {
            let from = Blocked;
            // pass: the assignee, split proven, evidence complete (no answer needed)
            assert!(
                check_status_transition(Some(from), Review, DEV, &split_ctx()).is_ok(),
                "{from:?}"
            );
            // refusals: not the assignee, a coordinator, no evidence
            let other = TicketCtx {
                actor_is_assignee: false,
                ..split_ctx()
            };
            assert_eq!(rule(Some(from), Review, DEV, &other), "status-wrong-writer");
            assert_eq!(
                rule(Some(from), Review, COORD, &split_ctx()),
                "status-wrong-writer"
            );
            let no_ev = TicketCtx {
                evidence_complete: false,
                ..split_ctx()
            };
            assert_eq!(
                rule(Some(from), Review, DEV, &no_ev),
                "status-evidence-missing"
            );
        }
    }

    #[test]
    fn without_the_split_proof_blocked_does_not_go_to_review() {
        let none = TicketCtx {
            split_off_proven: false,
            ..split_ctx()
        };
        assert_eq!(
            rule(Some(Blocked), Review, DEV, &none),
            "status-unknown-transition"
        );
        // the proof opens only the move to Review
        for to in [InProgress, Done, Ready, Backlog] {
            assert!(
                check_status_transition(Some(Blocked), to, DEV, &split_ctx()).is_err(),
                "{to:?}"
            );
        }
    }

    #[test]
    fn an_owner_merge_lets_the_coordinator_end_a_ticket_without_a_verify() {
        let merged = TicketCtx {
            done_reason: Some(DoneReason::OwnerMerged),
            verify: None,
            ..all_ok()
        };
        // pass: every status of an unfinished ticket, by the coordinator
        for from in [Review, InProgress, Blocked, Ready, Backlog] {
            assert!(
                check_status_transition(Some(from), Done, COORD, &merged).is_ok(),
                "{from:?}"
            );
        }
        // refused without the fact: the old rules stay
        let plain = TicketCtx {
            done_reason: None,
            ..merged
        };
        assert_eq!(
            rule(Some(Review), Done, COORD, &plain),
            "status-verify-required"
        );
        assert_eq!(
            rule(Some(Blocked), Done, COORD, &plain),
            "status-unknown-transition"
        );
        assert_eq!(
            rule(Some(InProgress), Done, COORD, &plain),
            "status-unknown-transition"
        );
        // refused with the fact: a dev agent, a ticket that is Done
        assert_eq!(
            rule(Some(Review), Done, DEV, &merged),
            "status-wrong-writer"
        );
        assert_eq!(rule(Some(Done), Done, COORD, &merged), "status-no-change");
        // the fact opens only Done
        assert_eq!(
            rule(Some(Review), InProgress, COORD, &merged),
            "status-verify-failed-required"
        );
    }

    #[test]
    fn closing_without_a_pr_drops_a_ticket_from_any_status_but_done_and_dropped() {
        let bare = TicketCtx {
            verify: None,
            ..all_ok()
        };
        // pass: every status of an unfinished ticket, by the coordinator only
        for from in [Review, InProgress, Blocked, Ready, Backlog] {
            assert!(
                check_status_transition(Some(from), Dropped, COORD, &bare).is_ok(),
                "{from:?}"
            );
            assert_eq!(
                rule(Some(from), Dropped, DEV, &bare),
                "status-wrong-writer",
                "{from:?}"
            );
        }
        // an ended ticket is not dropped: Done is final, Dropped is no change
        assert_eq!(rule(Some(Done), Dropped, COORD, &bare), "status-done-final");
        assert_eq!(
            rule(Some(Dropped), Dropped, COORD, &bare),
            "status-no-change"
        );
        // a new ticket is not dropped
        assert_eq!(
            rule(None, Dropped, COORD, &bare),
            "status-unknown-transition"
        );
        // the drop opens only Dropped: a close by judgment never reaches Done
        let r = super::check_status_transition(Some(Blocked), Done, COORD, &Transition::Drop)
            .unwrap_err();
        assert_eq!(r.rule, "status-unknown-transition");
    }

    #[test]
    fn dropped_is_final_for_everyone_but_the_owner() {
        for to in Status::ALL.into_iter().filter(|s| *s != Dropped) {
            for actor in [COORD, DEV] {
                assert_eq!(
                    rule(Some(Dropped), to, actor, &all_ok()),
                    "status-dropped-final",
                    "{to:?} by {actor:?}"
                );
            }
            assert!(
                check_status_transition(Some(Dropped), to, OWNER, &all_ok()).is_ok(),
                "{to:?}"
            );
        }
        // an owner merge does not end a Dropped ticket either
        let merged = TicketCtx {
            done_reason: Some(DoneReason::OwnerMerged),
            verify: None,
            ..all_ok()
        };
        assert_eq!(
            rule(Some(Dropped), Done, COORD, &merged),
            "status-dropped-final"
        );
        assert!(Done.ended() && Dropped.ended());
        for s in [Backlog, Ready, InProgress, Review, Blocked] {
            assert!(!s.ended(), "{s:?}");
        }
    }

    #[test]
    fn the_owner_moves_a_ticket_to_done_from_any_status_without_a_fact() {
        let bare = TicketCtx::default();
        for from in [Backlog, Ready, InProgress, Review, Blocked] {
            assert!(
                check_status_transition(Some(from), Done, OWNER, &bare).is_ok(),
                "{from:?}"
            );
        }
        // the owner is never refused, whatever reason a caller sets
        let judged = TicketCtx {
            done_reason: Some(DoneReason::OwnerMerged),
            ..bare
        };
        assert!(check_status_transition(Some(Review), Done, OWNER, &judged).is_ok());
    }

    #[test]
    fn a_verified_exit_needs_a_passed_verify_from_review_and_from_no_other_status() {
        for reason in [DoneReason::VerifiedClose, DoneReason::VerifiedMerge] {
            let ok = TicketCtx {
                done_reason: Some(reason),
                ..all_ok()
            };
            assert!(check_status_transition(Some(Review), Done, COORD, &ok).is_ok());
            let failed = TicketCtx {
                verify: Some(false),
                ..ok
            };
            assert_eq!(
                rule(Some(Review), Done, COORD, &failed),
                "status-verify-required"
            );
            // the verified exits skip nothing: no other status reaches Done
            for from in [Backlog, Ready, InProgress, Blocked] {
                assert_eq!(
                    rule(Some(from), Done, COORD, &ok),
                    "status-unknown-transition",
                    "{from:?}"
                );
            }
            assert_eq!(rule(Some(Review), Done, DEV, &ok), "status-wrong-writer");
        }
    }

    #[test]
    fn a_move_with_the_facts_of_another_move_is_not_in_the_table() {
        let close = Transition::Close {
            reason: Some(DoneReason::OwnerMerged),
            verify: Some(true),
        };
        // a reason to close is read only for a move to Done
        for (from, to) in [
            (Backlog, Ready),
            (InProgress, Review),
            (Blocked, InProgress),
        ] {
            let r = super::check_status_transition(Some(from), to, COORD, &close).unwrap_err();
            assert_eq!(r.rule, "status-unknown-transition", "{from:?} -> {to:?}");
        }
        let r = super::check_status_transition(None, Backlog, COORD, &close).unwrap_err();
        assert_eq!(r.rule, "status-unknown-transition");
    }

    #[test]
    fn a_skip_verify_reason_with_a_dev_actor_or_a_new_ticket_is_refused() {
        for reason in [DoneReason::OwnerMerged] {
            let c = TicketCtx {
                done_reason: Some(reason),
                verify: None,
                ..all_ok()
            };
            for from in [Review, InProgress, Blocked, Ready, Backlog] {
                assert_eq!(
                    rule(Some(from), Done, DEV, &c),
                    rule(
                        Some(from),
                        Done,
                        DEV,
                        &TicketCtx {
                            done_reason: None,
                            ..c
                        }
                    ),
                    "{reason:?} {from:?}"
                );
            }
            assert_eq!(rule(Some(Review), Done, DEV, &c), "status-wrong-writer");
            assert_eq!(rule(None, Done, COORD, &c), "status-unknown-transition");
        }
    }

    #[test]
    fn done_is_final_for_everyone_but_the_owner() {
        let facts: Vec<TicketCtx> = [
            None,
            Some(DoneReason::VerifiedClose),
            Some(DoneReason::VerifiedMerge),
            Some(DoneReason::OwnerMerged),
        ]
        .into_iter()
        .map(|done_reason| TicketCtx {
            done_reason,
            ..all_ok()
        })
        .collect();
        for ctx in &facts {
            for to in Status::ALL.into_iter().filter(|s| *s != Done) {
                for actor in [COORD, DEV] {
                    assert_eq!(
                        rule(Some(Done), to, actor, ctx),
                        "status-done-final",
                        "{to:?} by {actor:?}"
                    );
                }
                assert!(
                    check_status_transition(Some(Done), to, OWNER, ctx).is_ok(),
                    "{to:?}"
                );
            }
            // a repeat is no change, for the owner too
            for actor in [COORD, DEV, OWNER] {
                assert_eq!(rule(Some(Done), Done, actor, ctx), "status-no-change");
            }
        }
    }

    #[test]
    fn ask_is_for_the_coordinator_and_the_assignee_and_never_on_an_ended_ticket() {
        let key = "o/r#7";
        for status in [Backlog, Ready, InProgress, Review, Blocked] {
            assert!(check_ask(key, COORD, false, status).is_ok(), "{status:?}");
            assert!(check_ask(key, DEV, true, status).is_ok(), "{status:?}");
            let r = check_ask(key, DEV, false, status).unwrap_err();
            assert_eq!(r.rule, "ask-not-assignee");
            assert_eq!(
                r.message,
                "only the assignee or the coordinator asks the owner about o/r#7"
            );
        }
        for (actor, assignee) in [(COORD, false), (DEV, true), (DEV, false)] {
            assert_eq!(
                check_ask(key, actor, assignee, Done).unwrap_err().rule,
                "status-done-final"
            );
            assert_eq!(
                check_ask(key, actor, assignee, Dropped).unwrap_err().rule,
                "status-dropped-final"
            );
        }
    }

    #[test]
    fn ask_blocks_only_the_in_progress_ticket_of_the_assignee() {
        assert_eq!(ask_moves_to(InProgress, true), Some(Blocked));
        assert_eq!(ask_moves_to(InProgress, false), None);
        for status in [Backlog, Ready, Review, Blocked, Done] {
            assert_eq!(ask_moves_to(status, true), None, "{status:?}");
        }
    }

    #[test]
    fn an_open_question_refuses_verify_and_names_the_way_out() {
        assert!(check_no_open_question("o/r#7", None).is_ok());
        let r = check_no_open_question("o/r#7", Some("2026-10-20 10:00 UTC")).unwrap_err();
        assert_eq!(r.rule, "verify-owner-question-open");
        assert!(
            r.message
                .starts_with("o/r#7 has a question to the owner since 2026-10-20 10:00 UTC:"),
            "{}",
            r.message
        );
        assert!(
            r.message
                .contains("crewboard reassign 7 --to <agent> --reason"),
            "{}",
            r.message
        );
        assert!(r.message.ends_with("an answer in a chat is not signed"));
    }

    #[test]
    fn a_done_on_the_owner_route_is_not_verified() {
        assert!(check_not_owner_route("o/r#7", false).is_ok());
        let r = check_not_owner_route("o/r#7", true).unwrap_err();
        assert_eq!(r.rule, "verify-owner-route");
        assert!(
            r.message.ends_with("then crewboard close 7"),
            "{}",
            r.message
        );
    }
}
