//! Field writers (spec/board#fields, "One writer per field", spec/overview#where-things-live rule
//! 3).
//!
//! The table [`FIELD_RULES`] is the data; [`check_field_write`] is the function.
//! `Status` and `State` are not written here: they have transition tables of their own
//! (`status.rs`, `lifecycle.rs`).

use super::{Actor, Verdict, require};
use super::{Refusal, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Work,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Kind,
    Status,
    Agent,
    /// Optional queue position of a Ready ticket within its agent's queue (number; lowest first).
    Order,
    Gate,
    Evidence,
    Role,
    State,
    Heartbeat,
    Host,
    Runtime,
    Key,
    Doing,
    Cursor,
    /// Facts (detected) plus declared tags of an agent (spec/profiles).
    Capabilities,
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
    r(Item::Agent, Field::Doing, Writer::SelfOnly),
    r(Item::Agent, Field::Cursor, Writer::SelfOnly),
    r(Item::Agent, Field::Capabilities, Writer::SelfOrOwner),
];

/// Facts about one write attempt.
#[derive(Debug, Clone, Copy)]
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

/// Convenience for tests and callers: a context for an agent writing its own item.
impl WriteCtx {
    pub fn agent_self(role: Role) -> Self {
        WriteCtx {
            actor: Actor::Agent(role),
            is_subject: true,
            is_assigned: false,
            owner_approval: false,
            approved: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(actor: Actor) -> WriteCtx {
        WriteCtx {
            actor,
            is_subject: false,
            is_assigned: false,
            owner_approval: false,
            approved: false,
        }
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
