//! The fields of the board (spec/board#fields): the type [`Field`] names one, [`Field::name`]
//! is its name on the board, [`Field::kind`] its value type. The writer table that says who may
//! write a field is `rules::fields`; this module imports nothing.

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

/// The value type of a field on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    Text,
    SingleSelect,
    Number,
}

impl Field {
    /// Every field of spec/board#fields, in the order of the spec table.
    pub const ALL: [Field; 15] = [
        Field::Kind,
        Field::Status,
        Field::Agent,
        Field::Order,
        Field::Gate,
        Field::Evidence,
        Field::Role,
        Field::State,
        Field::Heartbeat,
        Field::Host,
        Field::Runtime,
        Field::Key,
        Field::Doing,
        Field::Cursor,
        Field::Capabilities,
    ];

    /// The name of the field on the board (the wire name). It never changes: the boards of
    /// every owner hold these names.
    pub const fn name(self) -> &'static str {
        match self {
            Field::Kind => "Kind",
            Field::Status => "Status",
            Field::Agent => "Agent",
            Field::Order => "Order",
            Field::Gate => "Gate",
            Field::Evidence => "Evidence",
            Field::Role => "Role",
            Field::State => "State",
            Field::Heartbeat => "Heartbeat",
            Field::Host => "Host",
            Field::Runtime => "Runtime",
            Field::Key => "Key",
            Field::Doing => "Doing",
            Field::Cursor => "Cursor",
            Field::Capabilities => "Capabilities",
        }
    }

    /// The value type that `init` creates the field with.
    pub const fn kind(self) -> FieldType {
        match self {
            Field::Kind | Field::Status | Field::Agent | Field::Role | Field::State => {
                FieldType::SingleSelect
            }
            Field::Order => FieldType::Number,
            Field::Gate
            | Field::Evidence
            | Field::Heartbeat
            | Field::Host
            | Field::Runtime
            | Field::Key
            | Field::Doing
            | Field::Cursor
            | Field::Capabilities => FieldType::Text,
        }
    }
}

impl std::fmt::Display for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}
