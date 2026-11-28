//! The scope, the budget and the stamp of the unattended coordinator
//! (spec/agents#unattended-coordinator), against the fake GitHub.
//!
//! In a headless config dir a judgment (`verify` of a Gate that is not `ci`, `verify --fail`, an
//! answer to the assignee, `unblock`, `assign --to`, `ask`) needs the mode of AUTHORITY.md. With
//! the mode `off` it is refused with `unattended-off` and nothing is written. With the mode
//! `verify` the command decides from the signed log of the ticket, counts against the budget of
//! the day and stamps its messages. An interactive config dir is not restricted.

use crate::fake_unattended::{
    GATE, OTHER, VERIFY, assign, by, coord, coord_lines, count_text, dev, done, hub, item_of, key,
    settle_day, stamp, state_path, tick, ticket, verdict,
};
use crate::support::board::{Out, REPO};
use crate::support::headless::{Hub, Scenario, TICKET};
use crewboard::protocol::acts::{reassigned_body, unattended_line};
use crewboard::protocol::names::Kind;
use crewboard::rules::Role;
use crewboard::state::StateCache;

const OFF: &str = "[rule=unattended-off]";
const BUDGET: &str = "[rule=unattended-budget]";
const SCOPE: &str = "[rule=headless-scope]";
const CI_ONLY: &str = "Goal: fix it\nGate: ci\n";
/// A ticket body that `assign` accepts.
const BODY: &str = "Goal: fix it\nGate: the demo runs\n";

/// The lines of the latest comment of the issue `n` that start with the key `unattended`.
fn stamps_of_last(h: &Hub, n: u64) -> Vec<String> {
    let (_, _, body) = h.board.comments_of(n).pop().expect("a comment");
    stamps_in(&body)
}

fn stamps_in(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.starts_with("unattended:"))
        .map(str::to_string)
        .collect()
}

/// The id of the latest comment of the issue `n`.
fn last_id(h: &Hub, n: u64) -> u64 {
    h.board.comments_of(n).pop().expect("a comment").0
}

fn refused(o: &Out, rule: &str) {
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains(rule), "wanted {rule}: {}", o.all());
}

fn passed(o: &Out) {
    assert_eq!(o.code, 0, "{}", o.all());
}

/// A new `done` of the dev agent on the ticket of the hub, which is in Review again. Each `done`
/// has its own text: two equal messages of one second are a message and its copy.
fn deliver_again(h: &Hub) {
    static ROUND: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let round = ROUND.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    done(h, TICKET, &format!("round: {round}\n"));
    h.board.set_item("PVTI_ticket", "Status", "Review");
}

/// A ticket that its agent blocked with a plain `blocked`.
fn blocked_ticket(h: &Hub, n: u64) {
    ticket(h, n, "Blocked", true);
    assign(h, n);
    dev(h, n, "coordinator", Kind::Blocked, "the API key is missing");
}

/// An issue with a body that has a `Gate:` line, its board item without an agent, and the signed
/// info comment of the coordinator that names the item.
fn free_ticket(h: &Hub, n: u64, status: &str) {
    h.board.issue_with_body(n, &["crew:work"], BODY);
    h.board
        .item(&item_of(n), &[("Status", status), ("Gate", GATE)]);
    coord(
        h,
        n,
        "owner",
        Kind::Info,
        &format!("Ticket is on the board.\nitem={}\nproject=15\n", item_of(n)),
    );
}

/// The feed reads of the verdict count among the requests from `from` on: the comment feed of
/// the repo from the start of the UTC day (the unread check reads the feed from the cursor).
fn feed_reads(h: &Hub, from: usize) -> usize {
    h.requests()[from..]
        .iter()
        .filter(|r| {
            r.method == "GET"
                && r.path.contains("/issues/comments?since=")
                && r.path.contains("T00:00:00Z")
        })
        .count()
}

/// `say --kind answer` to the dev agent of the hub.
fn answer<'a>(n: &'a str, to: &'a str, body: &'a str) -> Vec<&'a str> {
    vec![
        "say",
        n,
        "--to",
        to,
        "--kind",
        "answer",
        "--body",
        body,
        "--override-unread",
    ]
}

#[test]
fn with_the_key_off_every_judgment_is_refused_with_unattended_off_and_nothing_is_written() {
    for extra in ["", "unattended_coordinator: off\n"] {
        let h = hub();
        h.set_authority_extra(extra);
        blocked_ticket(&h, 6);
        free_ticket(&h, 12, "Backlog");
        assign(&h, 12);
        coord(&h, 12, &h.dev.name, Kind::Info, &reassigned_body("late"));
        let dev_name = h.dev.name.clone();
        let cases: Vec<Vec<&str>> = vec![
            // the Gate of #5 is prose: this verify is a judgment
            vec!["verify", "5"],
            vec!["verify", "5", "--body", "I ran the demo"],
            vec!["verify", "5", "--fail", "--body", "the demo does not run"],
            vec!["verify", "5", "--run-check", "tests"],
            answer("6", &dev_name, "Use the test key."),
            vec!["unblock", "6", "--override-unread"],
            vec!["ask", "6", "--body", "May it go?", "--override-unread"],
            vec!["assign", "12", "--to", &dev_name],
            // the command asks the mode first, whatever the ticket is
            vec!["unblock", "5", "--override-unread"],
            vec!["assign", "5", "--to", &dev_name],
        ];
        for argv in &cases {
            let from = h.requests().len();
            let o = h.headless(argv);
            refused(&o, OFF);
            assert!(
                o.stderr.contains("the unattended coordinator is off"),
                "{argv:?}: {}",
                o.all()
            );
            // the count of the day is not read for a mode that is off
            assert_eq!(feed_reads(&h, from), 0, "{argv:?}");
        }
        assert!(h.writes().is_empty(), "{extra:?}: {:?}", h.writes().len());
        assert_eq!(h.status_of_ticket().as_deref(), Some("Review"));
        assert_eq!(
            h.board.item_field(&item_of(6), "Status").as_deref(),
            Some("Blocked")
        );
        assert_eq!(h.board.item_field(&item_of(12), "Agent"), None);
        // the state holds nothing of the unattended coordinator
        let state = std::fs::read_to_string(state_path(&h)).unwrap_or_default();
        assert!(!state.contains("unattended"), "{state}");
    }
}

#[test]
fn with_the_key_off_a_headless_verify_of_a_ci_gate_is_as_it_was() {
    let h = Hub::new(Scenario {
        body: CI_ONLY,
        may_merge: true,
        ci_green: true,
    });
    let from = h.requests().len();
    let o = h.headless(&["verify", "5", "--body", "read again"]);
    passed(&o);
    assert!(o.stdout.starts_with("verify PASS"), "{}", o.all());
    assert_eq!(stamps_of_last(&h, TICKET), Vec::<String>::new());
    assert_eq!(feed_reads(&h, from), 0);
    // a failed Gate check is a judgment on every Gate
    let o = h.headless(&["verify", "5", "--fail", "--body", "the demo does not run"]);
    refused(&o, OFF);
    // and the merge of the CI-only ticket is as it was
    let o = h.headless(&["merge", "5"]);
    passed(&o);
    assert!(h.merged());
}

#[test]
fn an_unattended_verify_of_a_prose_gate_passes_is_stamped_and_moves_the_ticket_as_an_attended_one()
{
    settle_day();
    for fail in [false, true] {
        let argv: Vec<&str> = if fail {
            vec!["verify", "5", "--fail", "--body", "the demo does not run"]
        } else {
            vec!["verify", "5", "--body", "I ran the demo"]
        };
        // the attended coordinator, on a board of its own
        let attended = hub();
        attended.set_authority_extra(VERIFY);
        let a = attended.interactive(&argv);
        passed(&a);
        let (_, _, a_body) = attended.board.comments_of(TICKET).pop().unwrap();
        assert_eq!(stamps_in(&a_body), Vec::<String>::new(), "{a_body}");

        let h = hub();
        h.set_authority_extra(VERIFY);
        let o = h.headless(&argv);
        passed(&o);
        let word = if fail { "verify FAIL" } else { "verify PASS" };
        assert!(o.stdout.starts_with(word), "{}", o.all());
        assert!(a.stdout.starts_with(word), "{}", a.all());
        // the same Status as after the attended verify
        assert_eq!(h.status_of_ticket(), attended.status_of_ticket(), "{fail}");
        assert_eq!(
            h.status_of_ticket().as_deref(),
            Some(if fail { "In Progress" } else { "Review" })
        );
        // the message is the attended one with the stamp as its last body line
        let (_, _, body) = h.board.comments_of(TICKET).pop().unwrap();
        assert_eq!(stamps_in(&body), [unattended_line(1, 10)], "{body}");
        let line = if fail { "verify: fail" } else { "verify: pass" };
        assert!(body.contains(line) && a_body.contains(line), "{body}");
        let at = body.find("unattended: 1/10").unwrap();
        let before = &body[..at];
        assert!(
            before.contains("the demo") && before.ends_with('\n'),
            "{body}"
        );
        // the signed message counts: the tick reads it from the feed
        let t = tick(&h);
        assert_eq!(
            coord_lines(&t),
            [count_text("verify", 1, 10)],
            "{}",
            t.all()
        );
        // the floors are in the state, noted for the post
        let state = StateCache::load(&state_path(&h));
        assert_eq!(state.tick.unattended_floor.map(|f| f.1), Some(1));
        assert_eq!(state.tick.unattended_messages_floor.map(|f| f.1), Some(1));
        assert_eq!(
            state
                .tick
                .unattended_tickets
                .get(&key(TICKET))
                .map(|f| f.count),
            Some(1)
        );
    }
}

#[test]
fn the_unattended_verify_keeps_the_live_check_of_the_protected_paths() {
    let h = hub();
    h.set_authority_extra(VERIFY);
    // the `done` names no protected path, and the PR changes one now
    h.set_files(&[".github/workflows/ci.yml"]);
    let o = h.headless(&["verify", "5"]);
    refused(&o, "[rule=protected-path-owner-merge]");
    assert!(h.writes().is_empty(), "{:?}", h.writes().len());
    // a `done` on the owner route is not for the unattended coordinator at all
    let h = hub();
    h.set_authority_extra(VERIFY);
    done(&h, TICKET, "protected-paths: .github/workflows/ci.yml\n");
    let o = h.headless(&["verify", "5"]);
    refused(&o, SCOPE);
    assert!(o.stderr.contains("owner route"), "{}", o.all());
    assert!(h.writes().is_empty());
}

#[test]
fn the_verdict_after_the_budget_of_the_day_is_refused() {
    settle_day();
    let h = hub();
    h.set_authority_extra(VERIFY);
    // 9 stamped verdicts of today on another ticket
    ticket(&h, 6, "In Progress", true);
    assign(&h, 6);
    let d = done(&h, 6, "");
    for n in 1..=9 {
        verdict(&h, 6, d, &stamp(n));
    }
    // the 10th is the last one of the day
    let o = h.headless(&["verify", "5", "--fail", "--body", "the demo does not run"]);
    passed(&o);
    assert_eq!(stamps_of_last(&h, TICKET), [unattended_line(10, 10)]);
    // the 11th is refused, and nothing is written for it
    deliver_again(&h);
    let writes = h.writes().len();
    let o = h.headless(&["verify", "5"]);
    refused(&o, BUDGET);
    assert!(o.stderr.contains("10 of its 10 verdicts"), "{}", o.all());
    assert_eq!(h.writes().len(), writes);
    assert_eq!(h.status_of_ticket().as_deref(), Some("Review"));
    // an answer is no verdict: it still passes, with the count of the day in its stamp
    let dev_name = h.dev.name.clone();
    let o = h.headless(&answer("5", &dev_name, "The owner looks at it tomorrow."));
    passed(&o);
    assert_eq!(stamps_of_last(&h, TICKET), [unattended_line(10, 10)]);
    // a budget of the owner decides
    h.set_authority_extra("unattended_coordinator: verify\nunattended_budget: 11\n");
    let o = h.headless(&["verify", "5"]);
    passed(&o);
    assert_eq!(stamps_of_last(&h, TICKET), [unattended_line(11, 11)]);
}

#[test]
fn the_third_unattended_verdict_on_one_ticket_is_refused() {
    settle_day();
    let h = hub();
    h.set_authority_extra(VERIFY);
    for n in 1..=2 {
        let o = h.headless(&["verify", "5", "--fail", "--body", "the demo does not run"]);
        passed(&o);
        assert_eq!(stamps_of_last(&h, TICKET), [unattended_line(n, 10)]);
        deliver_again(&h);
    }
    let writes = h.writes().len();
    for argv in [
        vec!["verify", "5"],
        vec!["verify", "5", "--fail", "--body", "the demo does not run"],
    ] {
        let o = h.headless(&argv);
        refused(&o, BUDGET);
        assert!(
            o.stderr.contains("has its 2 unattended verdicts"),
            "{}",
            o.all()
        );
    }
    // the cap holds for the other judgments on the ticket too
    let dev_name = h.dev.name.clone();
    let o = h.headless(&answer("5", &dev_name, "Try again."));
    refused(&o, BUDGET);
    assert_eq!(h.writes().len(), writes);
    // an owner comment starts the count again (a unit test has the other cases: an attended
    // assignment does too, a reassign and an unattended assignment do not)
    h.board.owner_comment_now(TICKET, "I looked at it. Go on.");
    let o = h.headless(&["verify", "5"]);
    passed(&o);
    assert_eq!(stamps_of_last(&h, TICKET), [unattended_line(3, 10)]);
}

#[test]
fn a_deleted_and_an_edited_stamped_verdict_do_not_lower_the_counts_on_this_host() {
    settle_day();
    let h = hub();
    h.set_authority_extra(VERIFY);
    let fail = ["verify", "5", "--fail", "--body", "the demo does not run"];
    passed(&h.headless(&fail));
    let first = last_id(&h, TICKET);
    deliver_again(&h);
    passed(&h.headless(&fail));
    let second = last_id(&h, TICKET);
    deliver_again(&h);
    // a dev agent deletes one verdict and edits the other (the signature of an edited message
    // does not hold any more)
    h.board.delete_comment(first);
    let edited = h.board.comment_body(second).replace("unattended: 2/10", "");
    h.board.edit_comment_by(second, &edited, OTHER);
    // the ticket: the signed log has no stamped verdict left, and the floor of the state holds
    let o = h.headless(&["verify", "5"]);
    refused(&o, BUDGET);
    assert!(
        o.stderr.contains("has its 2 unattended verdicts"),
        "{}",
        o.all()
    );
    // the day: the tick still counts 2
    let t = tick(&h);
    assert_eq!(
        coord_lines(&t),
        [count_text("verify", 2, 10)],
        "{}",
        t.all()
    );
    // and a budget of 2 is spent
    ticket(&h, 6, "Review", true);
    assign(&h, 6);
    done(&h, 6, "");
    h.set_authority_extra("unattended_coordinator: verify\nunattended_budget: 2\n");
    let o = h.headless(&["verify", "6"]);
    refused(&o, BUDGET);
    assert!(o.stderr.contains("2 of its 2 verdicts"), "{}", o.all());
    // control: a host without the state counts what the log has left, so the floor is what
    // refused the commands above
    std::fs::remove_file(state_path(&h)).unwrap();
    let t = tick(&h);
    assert_eq!(coord_lines(&t), [count_text("verify", 0, 2)], "{}", t.all());
    // an owner comment ends the floor of the ticket
    let h2 = hub();
    h2.set_authority_extra(VERIFY);
    for _ in 0..2 {
        passed(&h2.headless(&fail));
        let id = last_id(&h2, TICKET);
        deliver_again(&h2);
        h2.board.delete_comment(id);
    }
    refused(&h2.headless(&["verify", "5"]), BUDGET);
    h2.board.owner_comment_now(TICKET, "I looked at it. Go on.");
    passed(&h2.headless(&["verify", "5"]));
}

#[test]
fn the_stamped_messages_of_a_day_have_a_fixed_cap() {
    settle_day();
    let h = hub();
    h.set_authority_extra(VERIFY);
    blocked_ticket(&h, 6);
    // 29 stamped answers of today on another ticket
    ticket(&h, 7, "In Progress", true);
    assign(&h, 7);
    for i in 0..29 {
        coord(
            &h,
            7,
            &h.dev.name,
            Kind::Answer,
            &format!("noted {i}\n{}", stamp(0)),
        );
    }
    // the 30th message passes
    let o = h.headless(&["ask", "5", "--body", "May it go?", "--override-unread"]);
    passed(&o);
    assert_eq!(stamps_of_last(&h, TICKET), [unattended_line(0, 10)]);
    // the 31st is refused, whatever the command is
    let writes = h.writes().len();
    let dev_name = h.dev.name.clone();
    for argv in [
        vec!["unblock", "6", "--override-unread"],
        answer("6", &dev_name, "Use the test key."),
        vec!["ask", "6", "--body", "May it go?", "--override-unread"],
        vec!["verify", "5"],
    ] {
        let o = h.headless(&argv);
        refused(&o, BUDGET);
        assert!(o.stderr.contains("its limit is 30 per day"), "{}", o.all());
    }
    assert_eq!(h.writes().len(), writes);
    // a deleted message does not lower the count of this host
    h.board.delete_comment(last_id(&h, TICKET));
    refused(&h.headless(&["unblock", "6", "--override-unread"]), BUDGET);
}

#[test]
fn a_count_that_cannot_be_read_refuses_the_command() {
    let h = hub();
    h.set_authority_extra(VERIFY);
    blocked_ticket(&h, 6);
    h.fail_get("/issues/comments?", 500);
    let dev_name = h.dev.name.clone();
    for argv in [
        vec!["verify", "5"],
        vec!["unblock", "6", "--override-unread"],
        answer("6", &dev_name, "Use the test key."),
        vec!["ask", "6", "--body", "May it go?", "--override-unread"],
    ] {
        let o = h.headless(&argv);
        refused(&o, BUDGET);
        assert!(
            o.stderr.contains("could not read its verdicts of today"),
            "{}",
            o.all()
        );
    }
    assert!(h.writes().is_empty(), "{:?}", h.writes().len());
    // the read works again, and so does the command
    h.fail_nothing();
    passed(&h.headless(&["verify", "5"]));
}

/// A ticket in Blocked that the round limit stopped: the failed verify with the `case:` line.
fn stopped_ticket(h: &Hub, n: u64) {
    ticket(h, n, "Blocked", true);
    assign(h, n);
    let d = done(h, n, "");
    verdict(h, n, d, "case: round limit 1 of 1; the owner decides\n");
}

#[test]
fn the_unattended_coordinator_never_takes_a_ticket_from_the_owner() {
    let h = hub();
    h.set_authority_extra(VERIFY);
    // 7: its agent asked the owner. 8: the round limit stopped it.
    blocked_ticket(&h, 7);
    dev(&h, 7, "owner", Kind::Question, "May I delete the old data?");
    stopped_ticket(&h, 8);
    let dev_name = h.dev.name.clone();
    for n in ["7", "8"] {
        for argv in [
            vec!["unblock", n, "--override-unread"],
            vec!["unblock", n, "--body", "go on", "--override-unread"],
            answer(n, &dev_name, "go on\nwithdraws: question"),
            answer(n, &dev_name, "go on"),
        ] {
            let o = h.headless(&argv);
            refused(&o, SCOPE);
        }
    }
    let o = h.headless(&["unblock", "7", "--override-unread"]);
    assert!(
        o.stderr.contains("has an open question to the owner"),
        "{}",
        o.all()
    );
    let o = h.headless(&["unblock", "8", "--override-unread"]);
    assert!(
        o.stderr.contains("is stopped at the round limit"),
        "{}",
        o.all()
    );
    // a withdrawing answer is refused on every ticket, also on one without a question
    blocked_ticket(&h, 6);
    let o = h.headless(&answer("6", &dev_name, "go on\nwithdraws: question"));
    refused(&o, SCOPE);
    assert!(o.stderr.contains("withdraws: question"), "{}", o.all());
    assert!(h.writes().is_empty(), "{:?}", h.writes().len());
    // control: the interactive coordinator withdraws the question of #7
    let o = h.interactive(&["unblock", "7", "--override-unread"]);
    passed(&o);
    assert!(
        o.stdout
            .contains("the open question to the owner is withdrawn"),
        "{}",
        o.all()
    );
    assert_eq!(stamps_of_last(&h, 7), Vec::<String>::new());
}

#[test]
fn unblock_and_an_answer_pass_for_a_ticket_that_waits_for_the_coordinator() {
    settle_day();
    let h = hub();
    h.set_authority_extra(VERIFY);
    blocked_ticket(&h, 6);
    blocked_ticket(&h, 9);
    let o = h.headless(&["unblock", "6", "--override-unread"]);
    passed(&o);
    assert!(o.stdout.contains("unblocked:"), "{}", o.all());
    let (_, _, body) = h.board.comments_of(6).pop().unwrap();
    assert_eq!(stamps_in(&body), [unattended_line(0, 10)], "{body}");
    assert!(!body.contains("withdraws: question"), "{body}");
    let dev_name = h.dev.name.clone();
    let o = h.headless(&answer("9", &dev_name, "Use the test key."));
    passed(&o);
    assert_eq!(stamps_of_last(&h, 9), [unattended_line(0, 10)]);
    // the stamp is the last body line, after the line of the unread override
    let (_, _, body) = h.board.comments_of(9).pop().unwrap();
    if let Some(at) = body.find("\noverride:") {
        assert!(body[at..].contains("\nunattended: 0/10"), "{body}");
    }
    // an answer to an agent that is not the assignee is refused
    let o = h.headless(&answer("9", OTHER, "Use the test key."));
    refused(&o, SCOPE);
    assert!(
        o.stderr.contains("goes to the assignee only"),
        "{}",
        o.all()
    );
    // no verdict was given: the count of the day is 0, and the messages are noted
    let t = tick(&h);
    assert_eq!(
        coord_lines(&t),
        [count_text("verify", 0, 10)],
        "{}",
        t.all()
    );
    let state = StateCache::load(&state_path(&h));
    assert_eq!(state.tick.unattended_messages_floor.map(|f| f.1), Some(2));
    // the unattended coordinator writes no heartbeat
    let hb = |h: &Hub| h.board.item_field(&h.coord.item_id, "Heartbeat");
    h.board
        .set_item(&h.coord.item_id, "Heartbeat", "2026-01-01T00:00:00Z");
    blocked_ticket(&h, 10);
    passed(&h.headless(&answer("10", &dev_name, "Use the test key.")));
    assert_eq!(hb(&h).as_deref(), Some("2026-01-01T00:00:00Z"));
    // control: the interactive coordinator writes one with the same command
    blocked_ticket(&h, 11);
    passed(&h.interactive(&answer("11", &dev_name, "Use the test key.")));
    assert_ne!(hb(&h).as_deref(), Some("2026-01-01T00:00:00Z"));
}

#[test]
fn a_board_field_that_contradicts_the_signed_log_gives_no_right() {
    let h = hub();
    h.set_authority_extra(VERIFY);
    let other = h.board.agent(OTHER, Role::Dev);
    // 6: assigned to the dev agent, which asked the owner. Another dev agent wrote its name
    // into the board Agent and posted a `blocked`.
    ticket(&h, 6, "Blocked", true);
    assign(&h, 6);
    dev(&h, 6, "owner", Kind::Question, "May I delete the old data?");
    h.board.set_item(&item_of(6), "Agent", OTHER);
    by(&h, &other, 6, "coordinator", Kind::Blocked, "I am stuck");
    let o = h.headless(&["unblock", "6", "--override-unread"]);
    refused(&o, "[rule=ticket-agent-not-signed]");
    let o = h.headless(&answer("6", OTHER, "go on"));
    refused(&o, SCOPE);
    assert!(
        o.stderr.contains("is not the agent of the signed log"),
        "{}",
        o.all()
    );
    // 7: the signed log says In Progress (the agent took it); somebody set Blocked on the board
    ticket(&h, 7, "Blocked", true);
    assign(&h, 7);
    dev(&h, 7, &h.coord.name, Kind::Ack, "I start now.");
    dev(&h, 7, "coordinator", Kind::Question, "Which port?");
    let o = h.headless(&["unblock", "7", "--override-unread"]);
    refused(&o, SCOPE);
    assert!(
        o.stderr.contains("the board Status of") && o.stderr.contains("the signed log says"),
        "{}",
        o.all()
    );
    // the ticket of the hub: somebody set Review on the board after a failed verify
    let d = h.board.comments_of(TICKET).pop().unwrap().0;
    verdict(&h, TICKET, d, "");
    let o = h.headless(&["verify", "5"]);
    refused(&o, SCOPE);
    assert!(h.writes().is_empty(), "{:?}", h.writes().len());
}

#[test]
fn assign_is_for_a_free_ticket_that_the_signed_log_knows() {
    settle_day();
    let h = hub();
    h.set_authority_extra(VERIFY);
    let dev_name = h.dev.name.clone();
    // 12: a reassign returned it to Backlog. 13: the owner moved it to Ready.
    free_ticket(&h, 12, "Backlog");
    assign(&h, 12);
    coord(&h, 12, &h.dev.name, Kind::Info, &reassigned_body("late"));
    free_ticket(&h, 13, "Ready");
    h.board.owner_comment(13, "/crew move Ready");
    // refused: each of these is not a free ticket of the signed log
    // 14: queued (Ready, with a signed assignment and its agent on the board)
    free_ticket(&h, 14, "Ready");
    h.board.set_item(&item_of(14), "Agent", &dev_name);
    assign(&h, 14);
    // 15: assigned in the signed log; the board says Ready without an Agent
    free_ticket(&h, 15, "Ready");
    assign(&h, 15);
    // 16: somebody set Ready on the board; the signed log says Backlog
    free_ticket(&h, 16, "Ready");
    // 17: in Backlog and never assigned
    free_ticket(&h, 17, "Backlog");
    // 18: an issue that is not on the board
    h.board.issue_with_body(18, &["crew:work"], BODY);
    assert!(h.board.comments_of(18).is_empty(), "{REPO}#18");
    // 19: stopped at the round limit; then somebody set Ready and cleared the Agent
    free_ticket(&h, 19, "Ready");
    assign(&h, 19);
    let d = done(&h, 19, "");
    verdict(&h, 19, d, "case: round limit 1 of 1; the owner decides\n");
    // 20: returned, and a question of a coordinator to the owner is open
    free_ticket(&h, 20, "Backlog");
    assign(&h, 20);
    coord(&h, 20, &h.dev.name, Kind::Info, &reassigned_body("late"));
    coord(&h, 20, "owner", Kind::Question, "Drop it?");
    for (n, why) in [
        ("14", "has an assignee in the signed log"),
        ("15", "has an assignee in the signed log"),
        ("16", "the board Status of"),
        ("17", "is not a Ready ticket without an agent"),
        ("18", "on the board"),
        ("19", "is stopped at the round limit"),
        ("20", "has an open question to the owner"),
    ] {
        let o = h.headless(&["assign", n, "--to", &dev_name]);
        refused(&o, SCOPE);
        assert!(o.stderr.contains(why), "#{n}: {}", o.all());
    }
    assert!(h.writes().is_empty(), "{:?}", h.writes().len());
    // the forms that the scope refuses before any request
    let from = h.requests().len();
    for argv in [
        vec!["assign", "12", "--to", &dev_name, "--order", "1"],
        vec![
            "assign",
            "12",
            "--to",
            &dev_name,
            "--override-capabilities",
            "--why",
            "x",
        ],
        vec!["assign", "14", "--order", "1"],
    ] {
        refused(&h.headless(&argv), SCOPE);
    }
    assert_eq!(h.requests().len(), from);
    // allowed: the two free tickets
    for n in [12, 13] {
        let o = h.headless(&["assign", &n.to_string(), "--to", &dev_name]);
        passed(&o);
        assert!(o.stdout.contains("Status Ready"), "{}", o.all());
        assert_eq!(
            h.board.item_field(&item_of(n), "Agent").as_deref(),
            Some(dev_name.as_str())
        );
        assert_eq!(
            h.board.item_field(&item_of(n), "Status").as_deref(),
            Some("Ready")
        );
        let (_, _, body) = h.board.comments_of(n).pop().unwrap();
        assert!(body.contains("Assigned to you:"), "{body}");
        assert_eq!(stamps_in(&body), [unattended_line(0, 10)], "{body}");
    }
    // the ticket has an assignee now: a second assign is refused
    let o = h.headless(&["assign", "12", "--to", &dev_name]);
    refused(&o, SCOPE);
}

#[test]
fn a_text_with_an_unattended_line_is_refused_in_a_headless_dir() {
    let h = hub();
    h.set_authority_extra(VERIFY);
    blocked_ticket(&h, 6);
    let dev_name = h.dev.name.clone();
    for text in [
        "fine\nunattended: 0/10",
        "unattended: 1/10\nfine",
        "fine\n  unattended: x",
    ] {
        for argv in [
            vec!["verify", "5", "--body", text],
            vec!["verify", "5", "--fail", "--body", text],
            answer("6", &dev_name, text),
            vec!["unblock", "6", "--body", text, "--override-unread"],
            vec!["ask", "6", "--body", text, "--override-unread"],
        ] {
            let o = h.headless(&argv);
            refused(&o, SCOPE);
            assert!(
                o.stderr.contains("only crewboard writes that line"),
                "{argv:?}: {}",
                o.all()
            );
        }
    }
    assert!(h.writes().is_empty(), "{:?}", h.writes().len());
    // a line of the tick is no stamp
    let o = h.headless(&answer(
        "6",
        &dev_name,
        "unattended coordinator: mode verify; verdicts 0/10 today",
    ));
    passed(&o);
    // the same with a Gate `ci` and the key off: no model writes the line
    let ci = Hub::new(Scenario {
        body: CI_ONLY,
        may_merge: true,
        ci_green: true,
    });
    let o = ci.headless(&["verify", "5", "--body", "fine\nunattended: 0/10"]);
    refused(&o, SCOPE);
    assert!(ci.writes().is_empty());
}

#[test]
fn the_tick_does_not_stamp_and_does_not_count_its_own_verify() {
    settle_day();
    let h = Hub::new(Scenario {
        body: CI_ONLY,
        may_merge: true,
        ci_green: true,
    });
    h.set_authority_extra(VERIFY);
    let o = tick(&h);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    assert!(h.merged(), "{}", o.all());
    for (_, _, body) in h.board.comments_of(TICKET) {
        assert_eq!(stamps_in(&body), Vec::<String>::new(), "{body}");
    }
    assert_eq!(
        coord_lines(&o),
        [count_text("verify", 0, 10)],
        "{}",
        o.all()
    );
    let state = StateCache::load(&state_path(&h));
    assert_eq!(state.tick.unattended_floor, None);
    assert_eq!(state.tick.unattended_messages_floor, None);
    assert!(state.tick.unattended_tickets.is_empty());
    // the command `verify` of a `ci` Gate is no judgment either
    let h = Hub::new(Scenario {
        body: CI_ONLY,
        may_merge: true,
        ci_green: true,
    });
    h.set_authority_extra(VERIFY);
    let from = h.requests().len();
    passed(&h.headless(&["verify", "5"]));
    assert_eq!(stamps_of_last(&h, TICKET), Vec::<String>::new());
    assert_eq!(feed_reads(&h, from), 0);
    let t = tick(&h);
    assert_eq!(
        coord_lines(&t),
        [count_text("verify", 0, 10)],
        "{}",
        t.all()
    );
}

#[test]
fn an_interactive_dir_is_not_restricted_and_gets_no_stamp_and_no_budget() {
    for extra in [
        "",
        VERIFY,
        "unattended_coordinator: verify\nunattended_budget: 1\n",
    ] {
        let h = hub();
        h.set_authority_extra(extra);
        blocked_ticket(&h, 6);
        blocked_ticket(&h, 7);
        dev(&h, 7, "owner", Kind::Question, "May I delete the old data?");
        free_ticket(&h, 12, "Backlog");
        free_ticket(&h, 13, "Backlog");
        // a spent budget and a spent ticket in the signed log
        let d = h.board.comments_of(TICKET).pop().unwrap().0;
        for n in 1..=2 {
            verdict(&h, 9, d, &stamp(n));
        }
        let dev_name = h.dev.name.clone();
        let from = h.requests().len();
        let cases: Vec<(u64, Vec<&str>)> = vec![
            (
                5,
                vec!["verify", "5", "--fail", "--body", "the demo does not run"],
            ),
            (
                6,
                answer("6", &dev_name, "Use the test key.\nunattended: 9/10"),
            ),
            (6, vec!["unblock", "6", "--override-unread"]),
            (
                6,
                vec!["ask", "6", "--body", "May it go?", "--override-unread"],
            ),
            // the interactive coordinator withdraws a question to the owner
            (7, vec!["unblock", "7", "--override-unread"]),
            // a ticket that no reassign returned, and a position in the queue
            (12, vec!["assign", "12", "--to", &dev_name]),
            (13, vec!["assign", "13", "--to", &dev_name, "--order", "5"]),
        ];
        for (n, argv) in &cases {
            let o = h.interactive(argv);
            assert_eq!(o.code, 0, "{extra:?} {argv:?}: {}", o.all());
            let (_, _, body) = h.board.comments_of(*n).pop().unwrap();
            // no stamp of crewboard; the line that the person typed stays as typed
            let typed = argv.iter().any(|a| a.contains("unattended: 9/10"));
            assert_eq!(
                stamps_in(&body),
                if typed {
                    vec!["unattended: 9/10".to_string()]
                } else {
                    Vec::new()
                },
                "{extra:?} {argv:?}: {body}"
            );
        }
        // no count of the day was read, and the state holds no floor
        assert_eq!(feed_reads(&h, from), 0, "{extra:?}");
        let name = crewboard::protocol::names::AgentName::parse(&h.coord.name).unwrap();
        let path = crewboard::config::ConfigDir(h.coord.dir.clone()).state_path("fake", &name);
        let state = std::fs::read_to_string(path).unwrap_or_default();
        assert!(!state.contains("unattended"), "{extra:?}: {state}");
    }
}

#[test]
fn the_other_mode_acts_as_verify_and_merges_no_prose_gate() {
    let h = hub();
    h.set_authority_extra("unattended_coordinator: verify-and-merge\n");
    let o = h.headless(&["verify", "5", "--body", "I ran the demo"]);
    passed(&o);
    assert_eq!(stamps_of_last(&h, TICKET), [unattended_line(1, 10)]);
    // the merge of a ticket whose Gate is not `ci` stays with the interactive coordinator
    let o = h.headless(&["merge", "5"]);
    refused(&o, SCOPE);
    assert!(!h.merged());
    let t = tick(&h);
    assert!(!h.merged(), "{}", t.all());
}
