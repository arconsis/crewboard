//! The option writer against a concurrent writer (spec/board#fields): the write replaces the
//! whole option list, so crewboard reads the list right before it and checks that the options it
//! created are still there after it. One retry; then a named refusal.

use crate::support::board::{FakeBoard, INITIATIVE};
use crate::support::cli::no_unhandled;

fn mutations(fake: &FakeBoard) -> usize {
    fake.requests()
        .iter()
        .filter(|r| r.body.contains("updateProjectV2Field("))
        .count()
}

#[test]
fn an_option_that_a_concurrent_write_took_away_is_written_again_once() {
    let fake = FakeBoard::new();
    let owner = fake.owner_dir();
    fake.seed_agent_options(&["hand-made-1".to_string()]);
    fake.joined("dev@fake-new");
    fake.drop_new_option_on_write(1);
    let o = fake.run(
        owner.path(),
        &["approve", "dev@fake-new", "--initiative", INITIATIVE],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    let names = fake.agent_options();
    assert!(names.contains(&"dev@fake-new".to_string()), "{names:?}");
    assert!(names.contains(&"hand-made-1".to_string()), "{names:?}");
    assert_eq!(mutations(&fake), 2, "one write and one retry");
    no_unhandled(&fake);
}

#[test]
fn a_second_loss_is_refused_by_name() {
    let fake = FakeBoard::new();
    let owner = fake.owner_dir();
    fake.joined("dev@fake-new");
    fake.drop_new_option_on_write(2);
    let o = fake.run(
        owner.path(),
        &["approve", "dev@fake-new", "--initiative", INITIATIVE],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("option-lost-concurrently") && o.stderr.contains("dev@fake-new"),
        "{}",
        o.all()
    );
    assert_eq!(mutations(&fake), 2);
}

#[test]
fn a_write_without_a_race_costs_one_more_read_and_no_retry() {
    let fake = FakeBoard::new();
    let owner = fake.owner_dir();
    fake.joined("dev@fake-new");
    let o = fake.run(
        owner.path(),
        &["approve", "dev@fake-new", "--initiative", INITIATIVE],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(mutations(&fake), 1);
    assert!(fake.agent_options().contains(&"dev@fake-new".to_string()));
}
