//! Field writers (spec/board#fields, "One writer per field", spec/overview#where-things-live rule
//! 3).
//!
//! The table [`FIELD_RULES`] is the data; [`check_field_write`] is the function.
//! `Status` and `State` are not written here: they have transition tables of their own
//! (`status.rs`, `lifecycle.rs`).
//!
//! The board writes a field only through `Board::write`, which asks [`check_field_write`]
//! first: "one writer per field" holds by construction. A field is a [`Field`], never a string,
//! so a misspelt field name does not compile. [`Field::name`] is the name on the board. The
//! type lives in `base::fields`, so a module that only reads fields needs no rule.

use super::{Actor, Verdict, require};
use super::{Refusal, Role};
pub use crate::base::fields::{Field, FieldType};
use crate::base::ids;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Work,
    Agent,
}

/// Who may write a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writer {
    /// Coordinator, or the owner (who can always edit tickets).
    CoordinatorOrOwner,
    /// The assigned dev agent, or the owner.
    AssignedDevOrOwner,
    /// Only the agent the item describes. Not even the owner: this stops impersonation.
    SelfOnly,
    /// Only when an owner approval comment exists (spec/board#coordinator-role).
    OwnerApproval,
    /// The agent itself while the item is created, frozen after approval (spec/board#fields).
    SelfUntilApproved,
    /// The agent the item describes, or the owner. Not another agent, not the coordinator
    /// (spec/profiles: profiles).
    SelfOrOwner,
    /// The agent the item describes. At a forced stop (`stop --force`) the owner or the
    /// coordinator writes it for the stopped agent, which cannot write any more
    /// (spec/board#lifecycle).
    SelfOrForcedStop,
    /// Written through a transition table, never directly.
    Transition,
}

pub struct FieldRule {
    pub item: Item,
    pub field: Field,
    pub writer: Writer,
}

const fn r(item: Item, field: Field, writer: Writer) -> FieldRule {
    FieldRule {
        item,
        field,
        writer,
    }
}

pub const FIELD_RULES: &[FieldRule] = &[
    r(Item::Work, Field::Kind, Writer::CoordinatorOrOwner),
    r(Item::Work, Field::Status, Writer::Transition),
    r(Item::Work, Field::Agent, Writer::CoordinatorOrOwner),
    r(Item::Work, Field::Order, Writer::CoordinatorOrOwner),
    r(Item::Work, Field::Gate, Writer::CoordinatorOrOwner),
    r(Item::Work, Field::Evidence, Writer::AssignedDevOrOwner),
    r(Item::Agent, Field::Kind, Writer::SelfOnly),
    r(Item::Agent, Field::Role, Writer::OwnerApproval),
    r(Item::Agent, Field::State, Writer::Transition),
    r(Item::Agent, Field::Heartbeat, Writer::SelfOnly),
    r(Item::Agent, Field::Host, Writer::SelfOnly),
    r(Item::Agent, Field::Runtime, Writer::SelfOnly),
    r(Item::Agent, Field::Key, Writer::SelfUntilApproved),
    r(Item::Agent, Field::Doing, Writer::SelfOrForcedStop),
    r(Item::Agent, Field::Cursor, Writer::SelfOnly),
    r(Item::Agent, Field::Capabilities, Writer::SelfOrOwner),
];

/// Facts about one write attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteCtx {
    pub actor: Actor,
    /// The item is an agent item and the actor is the agent it describes.
    pub is_subject: bool,
    /// The item is a ticket and the actor is the dev agent assigned to it.
    pub is_assigned: bool,
    /// An owner comment `/crew approve` or `/crew role` exists for this change.
    pub owner_approval: bool,
    /// The agent was approved, so its key is frozen.
    pub approved: bool,
    /// The write is part of a forced stop of the agent the item describes (`stop --force`).
    pub forced_stop: bool,
}

pub fn check_field_write(item: Item, field: Field, ctx: &WriteCtx) -> Verdict {
    let rule = FIELD_RULES
        .iter()
        .find(|r| r.item == item && r.field == field)
        .ok_or_else(|| {
            Refusal::new(
                "field-not-on-item",
                format!("{field:?} is not a field of {item:?} items"),
            )
        })?;
    let a = ctx.actor;
    match rule.writer {
        Writer::CoordinatorOrOwner => require(
            a.is_owner() || a.is_coordinator(),
            "field-wrong-writer",
            &format!("{field:?} may be written only by the coordinator or the owner"),
        ),
        Writer::AssignedDevOrOwner => require(
            a.is_owner() || (a.is_dev() && ctx.is_assigned),
            "field-wrong-writer",
            &format!("{field:?} may be written only by the assigned dev agent or the owner"),
        ),
        Writer::SelfOnly => require(
            !a.is_owner() && ctx.is_subject,
            "field-wrong-writer",
            &format!("{field:?} may be written only by the agent itself"),
        ),
        Writer::SelfOrForcedStop => require(
            (!a.is_owner() && ctx.is_subject)
                || (ctx.forced_stop && (a.is_owner() || a.is_coordinator())),
            "field-wrong-writer",
            &format!(
                "{field:?} may be written only by the agent itself, or by the owner or the coordinator at a forced stop"
            ),
        ),
        Writer::SelfOrOwner => require(
            a.is_owner() || ctx.is_subject,
            "field-wrong-writer",
            &format!("{field:?} may be written only by the agent itself or the owner"),
        ),
        Writer::OwnerApproval => require(
            ctx.owner_approval,
            "role-needs-owner-approval",
            "Role may be set only after an owner approval comment in the registry issue",
        ),
        Writer::SelfUntilApproved => {
            require(
                !ctx.approved,
                "key-frozen",
                "the Key is frozen after approval",
            )?;
            require(
                !a.is_owner() && ctx.is_subject,
                "field-wrong-writer",
                "Key may be written only by the agent itself, at join",
            )
        }
        Writer::Transition => Err(Refusal::new(
            "use-transition",
            format!("{field:?} is written only through its transition table"),
        )),
    }
}

impl WriteCtx {
    /// `actor` writes, with no other fact: not the subject, not assigned, no approval.
    pub const fn by(actor: Actor) -> Self {
        WriteCtx {
            actor,
            is_subject: false,
            is_assigned: false,
            owner_approval: false,
            approved: false,
            forced_stop: false,
        }
    }

    /// An agent with `role` writes its own item.
    pub const fn agent_self(role: Role) -> Self {
        WriteCtx {
            is_subject: true,
            ..WriteCtx::by(Actor::Agent(role))
        }
    }

    /// An agent writes its own item, and its role is not a fact of the write (a self-only
    /// field, or an agent before its approval). No self rule reads the role. `Dev` is the role
    /// with the fewest rights, so a use for any other field fails closed.
    pub const fn own_item() -> Self {
        WriteCtx::agent_self(Role::Dev)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(actor: Actor) -> WriteCtx {
        WriteCtx::by(actor)
    }
    const COORD: Actor = Actor::Agent(Role::Coordinator);
    const DEV: Actor = Actor::Agent(Role::Dev);

    fn rule_of(v: Verdict) -> &'static str {
        v.expect_err("must be refused").rule
    }

    #[test]
    fn table_has_one_row_per_item_and_field() {
        for (i, a) in FIELD_RULES.iter().enumerate() {
            for b in &FIELD_RULES[i + 1..] {
                assert!(
                    !(a.item == b.item && a.field == b.field),
                    "duplicate {:?}",
                    a.field
                );
            }
        }
    }

    #[test]
    fn gate_agent_kind_only_coordinator_or_owner() {
        for f in [Field::Gate, Field::Agent, Field::Kind] {
            assert!(check_field_write(Item::Work, f, &ctx(COORD)).is_ok());
            assert!(check_field_write(Item::Work, f, &ctx(Actor::Owner)).is_ok());
            assert_eq!(
                rule_of(check_field_write(Item::Work, f, &ctx(DEV))),
                "field-wrong-writer"
            );
        }
    }

    #[test]
    fn evidence_only_assigned_dev() {
        let mut c = ctx(DEV);
        c.is_assigned = true;
        assert!(check_field_write(Item::Work, Field::Evidence, &c).is_ok());
        c.is_assigned = false;
        assert_eq!(
            rule_of(check_field_write(Item::Work, Field::Evidence, &c)),
            "field-wrong-writer"
        );
        assert_eq!(
            rule_of(check_field_write(Item::Work, Field::Evidence, &ctx(COORD))),
            "field-wrong-writer"
        );
        assert!(check_field_write(Item::Work, Field::Evidence, &ctx(Actor::Owner)).is_ok());
    }

    #[test]
    fn self_fields_only_by_the_agent_itself() {
        for f in [
            Field::Heartbeat,
            Field::Host,
            Field::Runtime,
            Field::Doing,
            Field::Cursor,
        ] {
            assert!(
                check_field_write(Item::Agent, f, &WriteCtx::agent_self(Role::Dev)).is_ok(),
                "{f:?}"
            );
            // another agent
            assert_eq!(
                rule_of(check_field_write(Item::Agent, f, &ctx(DEV))),
                "field-wrong-writer"
            );
            // the coordinator does not write a dev's fields
            assert_eq!(
                rule_of(check_field_write(Item::Agent, f, &ctx(COORD))),
                "field-wrong-writer"
            );
            // the owner does not impersonate the agent
            let mut o = ctx(Actor::Owner);
            o.is_subject = true;
            assert_eq!(
                rule_of(check_field_write(Item::Agent, f, &o)),
                "field-wrong-writer"
            );
        }
    }

    #[test]
    fn capabilities_by_the_agent_itself_or_the_owner_never_by_another_agent() {
        // the agent itself
        assert!(
            check_field_write(
                Item::Agent,
                Field::Capabilities,
                &WriteCtx::agent_self(Role::Dev)
            )
            .is_ok()
        );
        assert!(
            check_field_write(
                Item::Agent,
                Field::Capabilities,
                &WriteCtx::agent_self(Role::Coordinator)
            )
            .is_ok()
        );
        // the owner, also for an agent that is not the owner
        assert!(check_field_write(Item::Agent, Field::Capabilities, &ctx(Actor::Owner)).is_ok());
        // another dev agent and the coordinator are refused
        for other in [DEV, COORD] {
            assert_eq!(
                rule_of(check_field_write(
                    Item::Agent,
                    Field::Capabilities,
                    &ctx(other)
                )),
                "field-wrong-writer"
            );
        }
        // the field exists on agent items only
        assert_eq!(
            rule_of(check_field_write(
                Item::Work,
                Field::Capabilities,
                &ctx(Actor::Owner)
            )),
            "field-not-on-item"
        );
    }

    #[test]
    fn role_needs_an_owner_approval() {
        let mut c = WriteCtx::agent_self(Role::Dev);
        assert_eq!(
            rule_of(check_field_write(Item::Agent, Field::Role, &c)),
            "role-needs-owner-approval"
        );
        c.owner_approval = true;
        assert!(check_field_write(Item::Agent, Field::Role, &c).is_ok());
    }

    #[test]
    fn key_writable_at_join_frozen_after_approval() {
        let mut c = WriteCtx::agent_self(Role::Dev);
        assert!(check_field_write(Item::Agent, Field::Key, &c).is_ok());
        assert_eq!(
            rule_of(check_field_write(Item::Agent, Field::Key, &ctx(DEV))),
            "field-wrong-writer"
        );
        c.approved = true;
        assert_eq!(
            rule_of(check_field_write(Item::Agent, Field::Key, &c)),
            "key-frozen"
        );
        // Frozen also for the owner.
        let mut o = ctx(Actor::Owner);
        o.approved = true;
        assert_eq!(
            rule_of(check_field_write(Item::Agent, Field::Key, &o)),
            "key-frozen"
        );
    }

    #[test]
    fn status_and_state_are_never_written_directly() {
        assert_eq!(
            rule_of(check_field_write(
                Item::Work,
                Field::Status,
                &ctx(Actor::Owner)
            )),
            "use-transition"
        );
        assert_eq!(
            rule_of(check_field_write(
                Item::Agent,
                Field::State,
                &ctx(Actor::Owner)
            )),
            "use-transition"
        );
    }

    #[test]
    fn doing_by_the_agent_itself_or_at_a_forced_stop_by_the_owner_or_the_coordinator() {
        assert!(check_field_write(Item::Agent, Field::Doing, &WriteCtx::own_item()).is_ok());
        for stopper in [Actor::Owner, COORD] {
            let mut c = ctx(stopper);
            assert_eq!(
                rule_of(check_field_write(Item::Agent, Field::Doing, &c)),
                "field-wrong-writer"
            );
            c.forced_stop = true;
            assert!(check_field_write(Item::Agent, Field::Doing, &c).is_ok());
        }
        // a dev agent cannot stop another agent, so the flag does not help it
        let mut d = ctx(DEV);
        d.forced_stop = true;
        assert_eq!(
            rule_of(check_field_write(Item::Agent, Field::Doing, &d)),
            "field-wrong-writer"
        );
        // the flag opens Doing only, not the other self fields
        let mut o = ctx(Actor::Owner);
        o.forced_stop = true;
        assert_eq!(
            rule_of(check_field_write(Item::Agent, Field::Heartbeat, &o)),
            "field-wrong-writer"
        );
    }

    #[test]
    fn every_field_has_one_rule_and_keeps_its_board_name() {
        let names: Vec<&str> = Field::ALL.iter().map(|f| f.name()).collect();
        assert_eq!(
            names,
            [
                "Kind",
                "Status",
                "Agent",
                "Order",
                "Gate",
                "Evidence",
                "Role",
                "State",
                "Heartbeat",
                "Host",
                "Runtime",
                "Key",
                "Doing",
                "Cursor",
                "Capabilities"
            ]
        );
        for f in Field::ALL {
            assert!(FIELD_RULES.iter().any(|r| r.field == f), "{f} has no rule");
            assert_eq!(f.to_string(), f.name());
        }
        assert_eq!(Field::Order.kind(), FieldType::Number);
        assert_eq!(Field::Status.kind(), FieldType::SingleSelect);
        assert_eq!(Field::Doing.kind(), FieldType::Text);
    }

    #[test]
    fn field_on_the_wrong_item_is_refused() {
        assert_eq!(
            rule_of(check_field_write(Item::Work, Field::Heartbeat, &ctx(COORD))),
            "field-not-on-item"
        );
        assert_eq!(
            rule_of(check_field_write(Item::Agent, Field::Gate, &ctx(COORD))),
            "field-not-on-item"
        );
    }
}
