use super::*;
use crate::protocol::names::Kind;
use crate::rules::Role;
use crate::testkit::agent;

fn msg(role: Role, kind: Kind, to: &str, body: String) -> VMsg {
    crate::testkit::msg(1, agent("x@host", role), to, Some(kind), &body)
}

const OLD: &str = "o/r#5";
const NEW: &str = "o/r#6";

fn body() -> String {
    format!("Goal.\n\n{}\n", split_line(NEW, "Open part"))
}

fn note(role: Role, kind: Kind, to: &str, new: &str) -> VMsg {
    msg(role, kind, to, split_note(OLD, new, "Open part"))
}

#[test]
fn the_split_proof_needs_the_line_and_the_note_of_a_coordinator_for_the_same_ticket() {
    let good = note(Role::Coordinator, Kind::Info, "b@host", NEW);
    // pass
    assert!(split_off_proven(
        &body(),
        std::slice::from_ref(&good),
        "b@host",
        OLD
    ));
    // refusals: each part alone, and each wrong part
    assert!(!split_off_proven(&body(), &[], "b@host", OLD), "line only");
    assert!(
        !split_off_proven("Goal.\n", std::slice::from_ref(&good), "b@host", OLD),
        "note only"
    );
    let by_dev = note(Role::Dev, Kind::Info, "b@host", NEW);
    assert!(!split_off_proven(&body(), &[by_dev], "b@host", OLD), "dev");
    let other_key = note(Role::Coordinator, Kind::Info, "b@host", "o/r#7");
    assert!(
        !split_off_proven(&body(), &[other_key], "b@host", OLD),
        "note for another ticket"
    );
    let other_kind = note(Role::Coordinator, Kind::Report, "b@host", NEW);
    assert!(!split_off_proven(&body(), &[other_kind], "b@host", OLD));
    assert!(
        !split_off_proven(&body(), std::slice::from_ref(&good), "c@host", OLD),
        "other agent"
    );
    let other_old = "o/r#9";
    assert!(!split_off_proven(&body(), &[good], "b@host", other_old));
    // a plain sentence in a dev message that looks like the note does not count
    let said = msg(
        Role::Dev,
        Kind::Info,
        "b@host",
        format!("The scope of {OLD} shrank. The open part moved to {NEW} (x)"),
    );
    assert!(!split_off_proven(&body(), &[said], "b@host", OLD));
}

#[test]
fn split_off_lines_read_what_split_line_writes() {
    let b = format!("a\n{}\nb\n", split_line("o/r#6", "Fix - and more"));
    assert_eq!(
        split_off_lines(&b),
        vec![("o/r#6".to_string(), "Fix - and more".to_string())]
    );
    assert!(split_off_lines("Split off: nothing\n").is_empty());
}
