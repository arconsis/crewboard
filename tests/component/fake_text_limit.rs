//! The text-field limit of the board against a fake GitHub.
//!
//! It replaces `it_text_fields_accept_1024_bytes_and_refuse_1025_without_writing`. The test
//! calls the library, not the CLI. It builds the client with `Client::for_base`, so it does
//! not touch the process environment and is safe next to other tests.

use crate::support::board::{FakeBoard, OWNER, PROJECT};
use crewboard::board::{Board, FieldValue, ItemRef};
use crewboard::github::{Client, Token};
use crewboard::rules::Actor;
use crewboard::rules::fields::{Field, WriteCtx};

#[test]
fn text_fields_accept_1024_bytes_and_refuse_1025_without_writing() {
    let fake = FakeBoard::new();
    let id = fake.item("PVTI_text", &[("Doing", "x")]);
    let client = Client::for_base(Token::new("ghp_faketoken"), &fake.base()).unwrap();
    let board = Board::load(&client, OWNER, PROJECT).unwrap();
    let writes = || {
        fake.requests()
            .iter()
            .filter(|r| r.body.contains("updateProjectV2ItemFieldValue"))
            .count()
    };
    let own = WriteCtx::own_item();
    let doing = |text: &str| {
        board.write(
            &client,
            ItemRef::agent(&id),
            Field::Doing,
            FieldValue::Text(text),
            &own,
        )
    };
    let read = || {
        board
            .read_item(&client, &id)
            .unwrap()
            .get(Field::Doing)
            .map(str::to_string)
    };

    let ok = "a".repeat(1024);
    doing(&ok).unwrap();
    assert_eq!(read().as_deref(), Some(ok.as_str()));
    assert_eq!(writes(), 1);

    let e = doing(&"a".repeat(1025)).unwrap_err();
    assert!(e.to_string().contains("text-field-too-long"), "{e}");
    assert_eq!(writes(), 1, "a refused value must not be sent");

    // two-byte characters: 512 pass (1024 bytes), 513 are refused
    doing(&"ä".repeat(512)).unwrap();
    assert_eq!(writes(), 2);
    assert!(doing(&"ä".repeat(513)).is_err());
    assert_eq!(writes(), 2, "a refused value must not be sent");
    // the refused write left the last good value
    assert_eq!(read().as_deref(), Some("ä".repeat(512).as_str()));
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn a_write_that_the_field_writer_table_refuses_sends_no_request() {
    let fake = FakeBoard::new();
    let id = fake.item("PVTI_rule", &[("Doing", "x")]);
    let client = Client::for_base(Token::new("ghp_faketoken"), &fake.base()).unwrap();
    let board = Board::load(&client, OWNER, PROJECT).unwrap();
    let writes = || {
        fake.requests()
            .iter()
            .filter(|r| r.body.contains("updateProjectV2ItemFieldValue"))
            .count()
    };
    // the owner does not write the Doing of an agent (only at a forced stop)
    let e = board
        .write(
            &client,
            ItemRef::agent(&id),
            Field::Doing,
            FieldValue::Text("impersonated"),
            &WriteCtx::by(Actor::Owner),
        )
        .unwrap_err();
    assert!(e.to_string().contains("field-wrong-writer"), "{e}");
    assert_eq!(writes(), 0, "a refused write must not be sent");
    // control: the agent itself writes it
    board
        .write(
            &client,
            ItemRef::agent(&id),
            Field::Doing,
            FieldValue::Text("own"),
            &WriteCtx::own_item(),
        )
        .unwrap();
    assert_eq!(writes(), 1);
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}
