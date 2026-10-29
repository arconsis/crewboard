//! Helpers that every fake-GitHub test file shares: run the CLI and check the exit code,
//! count the writes, make the usual crew.

use super::board::{Agent, FakeBoard, Out};
use crewboard::rules::Role;

/// The gate line of a ticket body that most tests use.
pub const GATE: &str = "**Gate:** the PR of this ticket is open and closes it.\n";

/// A board with a coordinator and one dev, both joined and approved.
pub fn crew() -> (FakeBoard, Agent, Agent) {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    (fake, c, b)
}

pub fn run_ok(fake: &FakeBoard, who: &Agent, args: &[&str]) -> Out {
    let o = fake.run(&who.dir, args);
    assert_eq!(o.code, 0, "{} {args:?}: {}", who.name, o.all());
    o
}

pub fn run_refused(fake: &FakeBoard, who: &Agent, args: &[&str], rule: &str) -> Out {
    let o = fake.run(&who.dir, args);
    assert_eq!(o.code, 2, "{} {args:?}: {}", who.name, o.all());
    assert!(
        o.stderr.contains(rule),
        "{rule} missing for {args:?}: {}",
        o.all()
    );
    o
}

/// The number of requests that write: not a GET, and not a read through GraphQL.
pub fn writes(fake: &FakeBoard) -> usize {
    fake.requests()
        .iter()
        .filter(|r| r.method != "GET" && (r.path != "/graphql" || r.body.contains("mutation")))
        .count()
}

/// A test that ends here proves the CLI made no call the fake does not know.
pub fn no_unhandled(fake: &FakeBoard) {
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

/// The item id that a `ticket new` style answer names (`added to the board as <id>;`).
pub fn item_in(o: &Out) -> String {
    o.stdout
        .split("added to the board as ")
        .nth(1)
        .unwrap_or_else(|| panic!("no item id in: {}", o.stdout))
        .split(';')
        .next()
        .unwrap()
        .to_string()
}
