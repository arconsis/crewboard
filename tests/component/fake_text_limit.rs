//! The text-field limit of the board against a fake GitHub.
//!
//! It replaces `it_text_fields_accept_1024_bytes_and_refuse_1025_without_writing`. The test
//! calls the library, not the CLI. It builds the client with `Client::for_base`, so it does
//! not touch the process environment and is safe next to other tests.

use crate::support::board::{FakeBoard, OWNER, PROJECT};
use crewboard::board::Board;
use crewboard::github::{Client, Token};

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
    let read = || {
        board
            .read_item(&client, &id)
            .unwrap()
            .field("Doing")
            .map(str::to_string)
    };

    let ok = "a".repeat(1024);
    board.set_text(&client, &id, "Doing", &ok).unwrap();
    assert_eq!(read().as_deref(), Some(ok.as_str()));
    assert_eq!(writes(), 1);

    let e = board
        .set_text(&client, &id, "Doing", &"a".repeat(1025))
        .unwrap_err();
    assert!(e.to_string().contains("text-field-too-long"), "{e}");
    assert_eq!(writes(), 1, "a refused value must not be sent");

    // two-byte characters: 512 pass (1024 bytes), 513 are refused
    board
        .set_text(&client, &id, "Doing", &"ä".repeat(512))
        .unwrap();
    assert_eq!(writes(), 2);
    assert!(
        board
            .set_text(&client, &id, "Doing", &"ä".repeat(513))
            .is_err()
    );
    assert_eq!(writes(), 2, "a refused value must not be sent");
    // the refused write left the last good value
    assert_eq!(read().as_deref(), Some("ä".repeat(512).as_str()));
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}
