//! The merge mode of the unattended coordinator and what the owner sees of it
//! (spec/agents#unattended-coordinator), against the fake GitHub.
//!
//! With `unattended_coordinator: verify-and-merge` the headless `tick` (and the command `merge`
//! in a headless config dir, by the same rule) merges a ticket whose Gate is prose, when the
//! latest `done` has a passing verify result for the head of its evidence. Every other check
//! of the merge stays. With `verify` and with `off` no prose Gate is merged. The tick tells the
//! owner once per UTC day that the verdicts are used, and the daily digest has a part about the
//! unattended coordinator.

use crate::fake_unattended::{
    BY_HAND, VERIFY, coord_lines, count_text, dev, done, hub, key, settle_day, state_path, tick,
    verdict,
};
use crate::support::board::{Out, REGISTRY, REPO};
use crate::support::headless::{Hub, PR, SHA, Scenario, TICKET};
use crewboard::clock::now_epoch;
use crewboard::protocol::acts::{is_verified_by_unattended, unattended_line};
use crewboard::protocol::names::Kind;
use crewboard::state::StateCache;

const MERGE: &str = "unattended_coordinator: verify-and-merge\n";
const SCOPE: &str = "[rule=headless-scope]";
const NOTICE: &str = "Merged by the coordinator.";

/// The id of the latest `done` of the dev agent on the ticket `n`.
fn latest_done(h: &Hub, n: u64) -> u64 {
    h.board
        .comments_of(n)
        .into_iter()
        .rev()
        .find(|(_, _, body)| body.contains("Done. Evidence follows."))
        .expect("a done")
        .0
}

/// The text of a passing verify result for the head [`SHA`], with `extra` lines after it.
fn pass_text(head_now: &str, extra: &str) -> String {
    format!(
        "verify: pass\npr: https://github.com/{REPO}/pull/{PR}\nsha: {SHA}\nhead_now: {head_now}\nci: https://github.com/{REPO}/actions/runs/9002\n{extra}"
    )
}

/// A passing verify result of the coordinator for the `done` in the comment `done_id`, created
/// `ago` seconds ago. `extra`: the lines after it (the stamp).
fn pass_ago(h: &Hub, n: u64, done_id: u64, extra: &str, ago: i64) -> u64 {
    h.board.signed_re_ago(
        &h.coord.key,
        &h.coord.name,
        &h.dev.name,
        Kind::Report,
        n,
        done_id,
        &pass_text(SHA, extra),
        ago,
    )
}

/// A stamped passing verify result for the latest `done` of the ticket of the hub.
fn stamped_pass(h: &Hub) -> u64 {
    pass_ago(
        h,
        TICKET,
        latest_done(h, TICKET),
        &format!("{}\n", unattended_line(1, 10)),
        0,
    )
}

/// Two ticks: the first one sees a new pass, the second one may merge it. The output of the
/// second one.
fn tick2(h: &Hub) -> Out {
    tick(h);
    tick(h)
}

/// The state of the headless dir as it is after two ticks saw the stamped pass `pass` on the
/// ticket of the hub, with the board item as it is now: the pass may give a merge. It lets a
/// test ask the command `merge` for its other refusals without a tick that writes.
fn seen_twice(h: &Hub, pass: u64) {
    use crewboard::state::{SeenPass, SeenResults};
    let path = state_path(h);
    let mut state = StateCache::load(&path);
    state.tick.review_results.insert(
        key(TICKET),
        SeenResults {
            ids: vec![pass],
            at: now_epoch(),
            pass: Some(SeenPass {
                id: pass,
                item_updated: h.board.item_updated_at("PVTI_ticket"),
                first: now_epoch(),
                confirmed: Some(now_epoch()),
            }),
            ..SeenResults::default()
        },
    );
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    state.save(&path).unwrap();
}

/// The left line of a tick for the ticket of the hub.
fn left_line(t: &Out) -> String {
    t.stdout
        .lines()
        .find(|l| l.starts_with("left for the interactive coordinator: "))
        .unwrap_or_else(|| panic!("no left line: {}", t.all()))
        .to_string()
}

fn refused(o: &Out, rule: &str) {
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains(rule), "wanted {rule}: {}", o.all());
}

/// The merge calls among the requests of the hub.
fn merge_calls(h: &Hub) -> usize {
    h.requests()
        .iter()
        .filter(|r| r.method == "PUT" && r.path.ends_with("/merge"))
        .count()
}

/// The latest comment of the ticket of the hub.
fn last_comment(h: &Hub) -> String {
    h.board.comments_of(TICKET).pop().expect("a comment").2
}

fn stamps_in(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|l| l.starts_with("unattended:"))
        .collect()
}

#[test]
fn in_the_merge_mode_the_tick_merges_after_a_stamped_pass_and_the_merge_counts_for_no_budget() {
    settle_day();
    let h = hub();
    h.set_authority_extra(MERGE);
    // the model of a wake gives the verdict: stamped and counted
    let o = h.headless_read(&["verify", "5", "--body", "I ran the demo"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(stamps_in(&last_comment(&h)), [unattended_line(1, 10)]);
    assert!(!h.merged());
    let before = StateCache::load(&state_path(&h)).tick;
    // the first tick sees the pass and merges nothing yet; the command `merge` neither
    let t = tick(&h);
    assert!(!h.merged(), "{}", t.all());
    assert!(
        left_line(&t).contains("not merged yet: the tick of this config dir did not see"),
        "{}",
        t.all()
    );
    let o = h.headless_read(&["merge", "5"]);
    refused(&o, SCOPE);
    assert!(
        o.stderr
            .contains("the tick merges the ticket at a later run"),
        "{}",
        o.all()
    );
    assert_eq!(merge_calls(&h), 0);
    // the second tick merges: no model takes part
    let t = tick(&h);
    assert!(h.merged(), "{}", t.all());
    assert_eq!(merge_calls(&h), 1);
    assert_eq!(h.status_of_ticket().as_deref(), Some("Done"));
    assert!(t.stdout.contains("merged at head"), "{}", t.all());
    // the notice says which verdict the merge followed, and it is no stamped message
    let notice = last_comment(&h);
    assert!(notice.contains(NOTICE), "{notice}");
    assert!(is_verified_by_unattended(&notice), "{notice}");
    assert_eq!(stamps_in(&notice), Vec::<&str>::new(), "{notice}");
    // the count of the day and the count of the ticket are what the verdict left
    assert_eq!(
        coord_lines(&t),
        [count_text("verify-and-merge", 1, 10)],
        "{}",
        t.all()
    );
    let after = StateCache::load(&state_path(&h)).tick;
    assert_eq!(after.unattended_floor, before.unattended_floor);
    assert_eq!(
        after.unattended_messages_floor,
        before.unattended_messages_floor
    );
    assert_eq!(after.unattended_tickets, before.unattended_tickets);
    // a second tick changes nothing
    let t = tick(&h);
    assert_eq!(merge_calls(&h), 1, "{}", t.all());
    assert_eq!(coord_lines(&t), [count_text("verify-and-merge", 1, 10)]);
}

#[test]
fn the_command_merge_of_a_headless_dir_follows_the_same_rule_as_the_tick() {
    let h = hub();
    h.set_authority_extra(MERGE);
    // no verify result yet: refused, and nothing is written
    let o = h.headless_read(&["merge", "5"]);
    refused(&o, SCOPE);
    assert!(
        o.stderr
            .contains("no passing verify result of the unattended coordinator"),
        "{}",
        o.all()
    );
    assert!(h.writes().is_empty());
    // a pass that no tick saw: the command waits for the tick, as the tick itself does
    stamped_pass(&h);
    let o = h.headless_read(&["merge", "5"]);
    refused(&o, SCOPE);
    assert!(o.stderr.contains("did not see"), "{}", o.all());
    assert!(h.writes().is_empty());
    // two ticks saw it, and the merge of the second one was refused for CI that still ran
    h.set_ci_pending(true);
    let t = tick2(&h);
    assert!(!h.merged(), "{}", t.all());
    assert!(left_line(&t).contains("merge refused: "), "{}", t.all());
    h.set_ci_pending(false);
    let o = h.headless_read(&["merge", "5"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(h.merged());
    let notice = last_comment(&h);
    assert!(is_verified_by_unattended(&notice), "{notice}");
    assert_eq!(stamps_in(&notice), Vec::<&str>::new(), "{notice}");
}

#[test]
fn the_mode_verify_and_the_mode_off_merge_no_prose_gate() {
    for extra in [VERIFY, "", "unattended_coordinator: off\n"] {
        let h = hub();
        h.set_authority_extra(extra);
        // a stamped pass and, after it, nothing that would stop a merge
        stamped_pass(&h);
        let o = h.headless_read(&["merge", "5"]);
        refused(&o, SCOPE);
        assert!(o.stderr.contains("it is not CI only"), "{}", o.all());
        let t = tick2(&h);
        assert!(!h.merged(), "{extra:?}: {}", t.all());
        assert_eq!(merge_calls(&h), 0, "{extra:?}");
        assert!(h.writes().is_empty(), "{extra:?}: {:?}", h.writes().len());
        assert_eq!(h.status_of_ticket().as_deref(), Some("Review"));
        assert!(
            t.stdout.contains(&format!(
                "left for the interactive coordinator: {}, the Gate is not CI only",
                key(TICKET)
            )),
            "{extra:?}: {}",
            t.all()
        );
    }
    // the same with the verdict of the command itself, in the mode `verify`
    let h = hub();
    h.set_authority_extra(VERIFY);
    let o = h.headless_read(&["verify", "5", "--body", "I ran the demo"]);
    assert_eq!(o.code, 0, "{}", o.all());
    refused(&h.headless_read(&["merge", "5"]), SCOPE);
    let t = tick2(&h);
    assert!(!h.merged(), "{}", t.all());
    assert_eq!(merge_calls(&h), 0);
    // the switch to the merge mode: the ticks of the mode `verify` saw the pass, so the next
    // tick merges
    h.set_authority_extra(MERGE);
    let t = tick(&h);
    assert!(h.merged(), "{}", t.all());
}

#[test]
fn an_attended_pass_is_merged_by_no_headless_dir_in_any_mode() {
    for extra in [MERGE, VERIFY, ""] {
        let h = hub();
        h.set_authority_extra(extra);
        // the interactive coordinator verifies and leaves the merge
        let o = h.interactive(&["verify", "5", "--body", "I ran the demo"]);
        assert_eq!(o.code, 0, "{}", o.all());
        assert_eq!(stamps_in(&last_comment(&h)), Vec::<&str>::new());
        let from = h.writes().len();
        let o = h.headless_read(&["merge", "5"]);
        refused(&o, SCOPE);
        if extra == MERGE {
            assert!(
                o.stderr
                    .contains("no passing verify result of the unattended coordinator"),
                "{}",
                o.all()
            );
        }
        let t = tick2(&h);
        assert!(!h.merged(), "{extra:?}: {}", t.all());
        assert_eq!(merge_calls(&h), 0, "{extra:?}");
        assert_eq!(h.writes().len(), from, "{extra:?}");
        assert_eq!(h.status_of_ticket().as_deref(), Some("Review"));
        // the interactive coordinator merges it, as it always did; its notice has no line
        let o = h.interactive(&["merge", "5"]);
        assert_eq!(o.code, 0, "{extra:?}: {}", o.all());
        assert!(h.merged());
        let notice = last_comment(&h);
        assert!(notice.contains(NOTICE), "{notice}");
        assert!(!is_verified_by_unattended(&notice), "{notice}");
    }
    // a person who merges after a pass of the unattended coordinator gets the line
    let p = hub();
    p.set_authority_extra(VERIFY);
    stamped_pass(&p);
    let o = p.interactive(&["merge", "5"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(is_verified_by_unattended(&last_comment(&p)));
}

/// A failed verify result of the coordinator for the latest `done` of the ticket of the hub,
/// posted as a comment only: the board does not move.
fn fail_after(h: &Hub) -> u64 {
    verdict(h, TICKET, latest_done(h, TICKET), "")
}

/// The interactive coordinator fails the latest `done` with the command: the result is posted
/// and the ticket leaves Review. Returns the comment id of the result.
fn real_fail(h: &Hub) -> u64 {
    let o = h.interactive(&[
        "verify",
        "5",
        "--fail",
        "--body",
        "[demo] the demo does not run",
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_ne!(
        h.status_of_ticket().as_deref(),
        Some("Review"),
        "{}",
        o.all()
    );
    h.board
        .comments_of(TICKET)
        .into_iter()
        .rev()
        .find(|(_, _, body)| body.contains("verify: fail"))
        .expect("the failed result")
        .0
}

/// What an account with write access can do after a fail: delete the result and set the board
/// back to Review.
fn hide_fail(h: &Hub, fail: u64) {
    h.board.delete_comment(fail);
    h.board.set_item("PVTI_ticket", "Status", "Review");
}

#[test]
fn a_pass_whose_ticket_left_review_at_a_tick_never_merges() {
    let h = hub();
    h.set_authority_extra(MERGE);
    let p = stamped_pass(&h);
    // CI still runs, so the two ticks see the pass and merge nothing
    h.set_ci_pending(true);
    let t = tick2(&h);
    assert!(!h.merged(), "{}", t.all());
    h.set_ci_pending(false);
    // a person fails the ticket: it leaves Review, and a tick sees it gone from Review
    let fail = real_fail(&h);
    tick(&h);
    let seen = StateCache::load(&state_path(&h)).tick.review_results;
    assert!(seen[&key(TICKET)].left.contains(&p), "{seen:?}");
    // the fail is deleted and the board is set back: the signed log shows the pass again
    hide_fail(&h, fail);
    for _ in 0..3 {
        let t = tick(&h);
        assert!(!h.merged(), "{}", t.all());
        let line = left_line(&t);
        assert!(line.contains("held: "), "{line}");
        assert!(
            line.contains(&format!("(comment {p}): that pass gives no headless merge")),
            "{line}"
        );
    }
    assert_eq!(merge_calls(&h), 0);
    let o = h.headless_read(&["merge", "5"]);
    refused(&o, SCOPE);
    assert!(o.stderr.contains("left Review"), "{}", o.all());
    // a new done with a new verdict is judged fresh
    done(&h, TICKET, "round: 2\n");
    stamped_pass(&h);
    let t = tick2(&h);
    assert!(h.merged(), "{}", t.all());
}

#[test]
fn a_fail_that_is_hidden_between_two_ticks_is_seen_by_the_change_of_the_board_item() {
    let h = hub();
    h.set_authority_extra(MERGE);
    let p = stamped_pass(&h);
    // the first tick sees the pass
    let t = tick(&h);
    assert!(!h.merged(), "{}", t.all());
    // before the next tick: a fail, its deletion, and the board back to Review
    let fail = real_fail(&h);
    hide_fail(&h, fail);
    let t = tick2(&h);
    assert!(!h.merged(), "{}", t.all());
    assert_eq!(merge_calls(&h), 0);
    assert!(
        left_line(&t).contains(&format!(
            "held: {} left Review, or its board item changed",
            key(TICKET)
        )),
        "{}",
        t.all()
    );
    let seen = StateCache::load(&state_path(&h)).tick.review_results;
    assert!(seen[&key(TICKET)].left.contains(&p), "{seen:?}");
}

/// The window that stays, as the docs name it: the pass, a fail that moves the ticket, the
/// deletion of the fail and the move back to Review all happen before the first tick after the
/// pass. No tick saw the pass before, the signed log shows only the pass, and the board item
/// has no earlier value to compare. The pass merges.
#[test]
fn a_fail_that_is_hidden_before_the_first_tick_after_the_pass_is_not_detected() {
    let h = hub();
    h.set_authority_extra(MERGE);
    stamped_pass(&h);
    let fail = real_fail(&h);
    hide_fail(&h, fail);
    let t = tick2(&h);
    assert!(h.merged(), "{}", t.all());
}

#[test]
fn an_edited_fail_after_a_stamped_pass_holds_the_headless_merge() {
    // an edit that takes the crew header away, by another account than the author: an edited
    // comment that is no verified message
    let plain = hub();
    plain.set_authority_extra(MERGE);
    let p = stamped_pass(&plain);
    seen_twice(&plain, p);
    let fail = fail_after(&plain);
    plain
        .board
        .edit_comment_by(fail, "nothing to see", "somebody-else");
    refused(&plain.headless_read(&["merge", "5"]), SCOPE);
    let t = tick2(&plain);
    assert!(!plain.merged(), "{}", t.all());

    let h = hub();
    h.set_authority_extra(MERGE);
    let p = stamped_pass(&h);
    seen_twice(&h, p);
    let fail = fail_after(&h);
    // somebody with write access edits the fail and keeps its header: it is no verified
    // message any more, and the signed log shows the pass as the latest result
    let text = h.board.comment_body(fail);
    h.board.edit_comment(fail, &format!("{text}\nmore"));
    let o = h.headless_read(&["merge", "5"]);
    refused(&o, SCOPE);
    assert!(
        o.stderr
            .contains(&format!("comment {fail} of {}", key(TICKET))),
        "{}",
        o.all()
    );
    assert!(o.stderr.contains("is no verified message"), "{}", o.all());
    let t = tick2(&h);
    assert!(!h.merged(), "{}", t.all());
    assert_eq!(merge_calls(&h), 0);
    assert!(
        left_line(&t).contains(&format!("held: comment {fail} of {}", key(TICKET))),
        "{}",
        t.all()
    );
    // the interactive coordinator reads the ticket and decides
    let o = h.interactive(&["merge", "5"]);
    assert_eq!(o.code, 0, "{}", o.all());
}

/// A false positive of the check for unverified comments, which the spec names: a signed
/// message of an agent that is valid in every way but its time (outside the replay window).
/// It holds the merge, and the line names the comment.
#[test]
fn a_valid_message_outside_the_replay_window_holds_the_merge_and_is_named() {
    let h = hub();
    h.set_authority_extra(MERGE);
    stamped_pass(&h);
    let odd = h.board.signed_with_ts(
        &h.dev.key,
        &h.dev.name,
        &h.coord.name,
        Kind::Info,
        TICKET,
        "a note with an old time",
        now_epoch() - 3 * 86_400,
    );
    let t = tick2(&h);
    assert!(!h.merged(), "{}", t.all());
    assert!(
        left_line(&t).contains(&format!("held: comment {odd} of {}", key(TICKET))),
        "{}",
        t.all()
    );
    assert_eq!(merge_calls(&h), 0);
}

#[test]
fn a_result_that_the_tick_saw_and_that_is_gone_holds_the_ticket_until_a_release() {
    settle_day();
    let h = hub();
    h.set_authority_extra(MERGE);
    // two stamped passes for one done; CI still runs, so the ticks see both and merge nothing
    let first = stamped_pass(&h);
    let second = pass_ago(
        &h,
        TICKET,
        latest_done(&h, TICKET),
        &format!("checked again\n{}\n", unattended_line(2, 10)),
        0,
    );
    h.set_ci_pending(true);
    tick2(&h);
    h.set_ci_pending(false);
    // a deleted PASS: the older one is the latest result now
    h.board.delete_comment(second);
    let t = tick2(&h);
    assert!(!h.merged(), "{}", t.all());
    assert!(
        left_line(&t).contains(&format!("(comment {second}) is gone or was edited")),
        "{}",
        t.all()
    );
    let noted = StateCache::load(&state_path(&h)).tick.review_results[&key(TICKET)].clone();
    assert_eq!(noted.gone.len(), 1, "{noted:?}");
    assert_eq!(noted.gone[0].0, second);
    assert!(noted.ids.contains(&first));
    refused(&h.headless_read(&["merge", "5"]), SCOPE);
    // a new done with a new stamped pass does not end the hold
    done(&h, TICKET, "round: 2\n");
    stamped_pass(&h);
    let t = tick2(&h);
    assert!(!h.merged(), "{}", t.all());
    assert!(
        left_line(&t).contains("is gone or was edited"),
        "{}",
        t.all()
    );
    assert_eq!(merge_calls(&h), 0);
    // a comment of the owner after the time of the note releases it; the ticks saw the new
    // pass twice already, so the next one merges
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    h.board.owner_comment_now(TICKET, "I looked at it. Go on.");
    let t = tick(&h);
    assert!(h.merged(), "{}", t.all());
}

#[test]
fn the_tick_notes_what_it_sees_in_every_mode_that_is_not_off() {
    for (extra, notes) in [("", false), (VERIFY, true), (MERGE, true)] {
        let h = hub();
        h.set_authority_extra(extra);
        let p = stamped_pass(&h);
        fail_after(&h);
        tick(&h);
        let state = std::fs::read_to_string(state_path(&h)).unwrap_or_default();
        assert_eq!(
            state.contains("review_results"),
            notes,
            "{extra:?}: {state}"
        );
        if notes {
            let seen = StateCache::load(&state_path(&h)).tick.review_results;
            assert!(seen[&key(TICKET)].ids.contains(&p), "{extra:?}: {seen:?}");
        }
    }
}

#[test]
fn the_digest_marks_a_ticket_that_is_held() {
    let h = hub();
    h.set_authority_extra(&format!("{MERGE}daily_digest: true\n"));
    let p = stamped_pass(&h);
    seen_twice(&h, p);
    // the ticket left Review at an earlier tick
    let path = state_path(&h);
    let mut state = StateCache::load(&path);
    state
        .tick
        .review_results
        .get_mut(&key(TICKET))
        .unwrap()
        .left = vec![p];
    state.save(&path).unwrap();
    let t = tick(&h);
    assert!(!h.merged(), "{}", t.all());
    let body = digest_body(&h);
    assert!(
        body.contains(&format!(
            "Verified by the unattended coordinator, in Review and not merged: 1 ({} held)",
            key(TICKET)
        )),
        "{body}"
    );
    // the list has the key and the word; the reason of the hold is in the line of the ticket
    // among those that wait for a person, in the words of the tool
    assert!(
        body.contains(&format!(
            "- {}: held: {} left Review",
            key(TICKET),
            key(TICKET)
        )),
        "{body}"
    );
}

/// One reason that must still refuse the merge in the mode `verify-and-merge`.
struct Case {
    name: &'static str,
    scenario: fn() -> Scenario<'static>,
    /// Runs after the hub exists and before the stamped pass.
    before: fn(&Hub),
    /// Runs after the stamped pass.
    after: fn(&Hub),
    /// A part of the refusal of the command `merge`.
    refusal: &'static str,
}

fn plain() -> Scenario<'static> {
    Scenario {
        body: BY_HAND,
        may_merge: true,
        ci_green: true,
    }
}

fn nothing(_: &Hub) {}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "red CI",
            scenario: || Scenario {
                ci_green: false,
                ..plain()
            },
            before: nothing,
            after: nothing,
            refusal: "counted CI run that is green",
        },
        Case {
            name: "pending CI",
            scenario: plain,
            before: nothing,
            after: |h| h.set_ci_pending(true),
            refusal: "counted CI run that is green",
        },
        Case {
            name: "coordinator_may_merge: false",
            scenario: || Scenario {
                may_merge: false,
                ..plain()
            },
            before: nothing,
            after: nothing,
            refusal: "coordinator_may_merge is false",
        },
        Case {
            name: "a protected path",
            scenario: plain,
            before: nothing,
            after: |h| {
                h.set_authority_extra(&format!("{MERGE}protected_paths: [tests/**]\n"));
                h.set_files(&["src/it.rs", "tests/ci.toml"]);
            },
            refusal: "[rule=protected-path-owner-merge]",
        },
        Case {
            name: "a guarded path",
            scenario: plain,
            before: |h| h.sign_assignment("gate: the demo runs\nci: required\nguard: tests/**\n"),
            after: |h| h.set_files(&["src/it.rs", "tests/ci.toml"]),
            refusal: "[rule=guarded-path-changed]",
        },
        Case {
            name: "a base branch that hides a protected file",
            scenario: plain,
            before: nothing,
            after: |h| h.set_base("stage", &[".github/workflows/ci.yml"]),
            refusal: "[rule=protected-path-owner-merge]",
        },
        Case {
            name: "a base branch that hides a guarded file",
            scenario: plain,
            before: |h| h.sign_assignment("gate: the demo runs\nci: required\nguard: tests/**\n"),
            after: |h| h.set_base("stage", &["tests/ci.toml"]),
            refusal: "[rule=guarded-path-changed]",
        },
        Case {
            name: "CI: none",
            scenario: || Scenario {
                body: "Goal: fix it\nGate: the demo runs\nCI: none\n",
                ..plain()
            },
            before: nothing,
            after: nothing,
            refusal: "declares `CI: none`",
        },
        Case {
            name: "an open question to the owner",
            scenario: plain,
            before: nothing,
            after: |h| {
                dev(h, TICKET, "owner", Kind::Question, "May I delete the data?");
            },
            refusal: "has an open question to the owner",
        },
        Case {
            name: "a stop at the round limit",
            scenario: plain,
            before: |h| {
                let d = latest_done(h, TICKET);
                verdict(
                    h,
                    TICKET,
                    d,
                    "case: round limit 1 of 1; the owner decides\n",
                );
                done(h, TICKET, "round: 2\n");
            },
            after: nothing,
            refusal: "no passing verify result of the unattended coordinator",
        },
        Case {
            name: "a moved head",
            scenario: plain,
            before: nothing,
            after: |h| h.set_head("ffffffffffffffffffffffffffffffffffffffff"),
            refusal: "[rule=evidence-head-moved]",
        },
        Case {
            name: "the owner route",
            scenario: plain,
            before: |h| {
                done(h, TICKET, "owner-route: docs/patches/5-workflow.patch\n");
            },
            after: nothing,
            refusal: "is on the owner route",
        },
        Case {
            name: "a done that names a protected path",
            scenario: plain,
            before: |h| {
                done(h, TICKET, "protected-paths: .github/workflows/ci.yml\n");
            },
            after: nothing,
            refusal: "no passing verify result of the unattended coordinator",
        },
        Case {
            name: "a later done",
            scenario: plain,
            before: nothing,
            after: |h| {
                done(h, TICKET, "round: 2\n");
            },
            refusal: "no passing verify result of the unattended coordinator",
        },
        Case {
            name: "a failed result after the pass",
            scenario: plain,
            before: nothing,
            after: |h| {
                let d = latest_done(h, TICKET);
                verdict(h, TICKET, d, "");
            },
            refusal: "no passing verify result of the unattended coordinator",
        },
    ]
}

#[test]
fn every_other_refusal_of_the_merge_still_fires_in_the_merge_mode() {
    // control: the setup of these cases merges when no reason is added
    let ok = Hub::new(plain());
    ok.set_authority_extra(MERGE);
    stamped_pass(&ok);
    let t = tick2(&ok);
    assert!(ok.merged(), "{}", t.all());
    // and the command `merge` passes with the state that these cases give it
    let ok = Hub::new(plain());
    ok.set_authority_extra(MERGE);
    let p = stamped_pass(&ok);
    seen_twice(&ok, p);
    let o = ok.headless_read(&["merge", "5"]);
    assert_eq!(o.code, 0, "{}", o.all());

    for c in cases() {
        // the command `merge`, and the tick, each on a board of its own
        for by_tick in [false, true] {
            let h = Hub::new((c.scenario)());
            h.set_authority_extra(MERGE);
            (c.before)(&h);
            let p = stamped_pass(&h);
            if !by_tick {
                seen_twice(&h, p);
            }
            (c.after)(&h);
            if by_tick {
                let t = tick2(&h);
                assert!(!h.merged(), "{}: {}", c.name, t.all());
                assert_eq!(merge_calls(&h), 0, "{}", c.name);
                assert_ne!(
                    h.status_of_ticket().as_deref(),
                    Some("Done"),
                    "{}: {}",
                    c.name,
                    t.all()
                );
            } else {
                let o = h.headless_read(&["merge", "5"]);
                assert_eq!(o.code, 2, "{}: {}", c.name, o.all());
                assert!(
                    o.stderr.contains("refused [rule="),
                    "{}: {}",
                    c.name,
                    o.all()
                );
                assert!(
                    o.stderr.contains(c.refusal),
                    "{}: wanted {:?}: {}",
                    c.name,
                    c.refusal,
                    o.all()
                );
                assert!(!h.merged(), "{}", c.name);
                assert!(h.writes().is_empty(), "{}: {:?}", c.name, h.writes().len());
            }
        }
    }
}

#[test]
fn a_dev_agent_cannot_make_a_pass_and_a_result_for_another_head_is_none() {
    // a "verify result" that the dev agent signs, with the line of the stamp
    let h = hub();
    h.set_authority_extra(MERGE);
    let d = latest_done(&h, TICKET);
    h.board.signed_re_ago(
        &h.dev.key,
        &h.dev.name,
        &h.coord.name,
        Kind::Report,
        TICKET,
        d,
        &pass_text(SHA, &format!("{}\n", unattended_line(1, 10))),
        0,
    );
    refused(&h.headless_read(&["merge", "5"]), SCOPE);
    let t = tick2(&h);
    assert!(!h.merged(), "{}", t.all());
    assert_eq!(merge_calls(&h), 0);

    // the coordinator saw another head than the evidence names
    let h = hub();
    h.set_authority_extra(MERGE);
    let d = latest_done(&h, TICKET);
    h.board.signed_re_ago(
        &h.coord.key,
        &h.coord.name,
        &h.dev.name,
        Kind::Report,
        TICKET,
        d,
        &pass_text(
            "ffffffffffffffffffffffffffffffffffffffff",
            &format!("{}\n", unattended_line(1, 10)),
        ),
        0,
    );
    refused(&h.headless_read(&["merge", "5"]), SCOPE);
    let t = tick2(&h);
    assert!(!h.merged(), "{}", t.all());

    // a line with the key that is not valid is no stamp and no attended result
    let h = hub();
    h.set_authority_extra(MERGE);
    let d = latest_done(&h, TICKET);
    pass_ago(&h, TICKET, d, "unattended: many\n", 0);
    refused(&h.headless_read(&["merge", "5"]), SCOPE);
    let t = tick2(&h);
    assert!(!h.merged(), "{}", t.all());

    // an old pass does not count for a new done, and the new verdict does
    let h = hub();
    h.set_authority_extra(MERGE);
    stamped_pass(&h);
    done(&h, TICKET, "round: 2\n");
    let t = tick2(&h);
    assert!(!h.merged(), "{}", t.all());
    stamped_pass(&h);
    let t = tick2(&h);
    assert!(h.merged(), "{}", t.all());
}

#[test]
fn a_gate_of_checks_is_not_merged_in_the_merge_mode() {
    let h = Hub::new(Scenario {
        body: "Goal: fix it\nGate: checks: ara\n",
        may_merge: true,
        ci_green: true,
    });
    h.set_authority_extra(MERGE);
    let d = latest_done(&h, TICKET);
    pass_ago(&h, TICKET, d, &format!("{}\n", unattended_line(1, 10)), 0);
    let o = h.headless_read(&["merge", "5"]);
    refused(&o, SCOPE);
    assert!(o.stderr.contains("it is not CI only"), "{}", o.all());
    let t = tick2(&h);
    assert!(!h.merged(), "{}", t.all());
    assert_eq!(merge_calls(&h), 0);
}

/// The notes in the registry issue that say the verdicts of a day are used.
fn spent_notes(h: &Hub) -> Vec<String> {
    h.board
        .comments_of(REGISTRY)
        .into_iter()
        .map(|(_, _, body)| body)
        .filter(|b| b.contains("event=unattended-budget:"))
        .collect()
}

#[test]
fn the_tick_tells_the_owner_once_per_day_that_the_verdicts_are_used() {
    settle_day();
    let h = hub();
    h.set_authority_extra("unattended_coordinator: verify\nunattended_budget: 1\n");
    // a budget with room: no note
    let t = tick(&h);
    assert!(spent_notes(&h).is_empty(), "{}", t.all());
    let o = h.headless_read(&["verify", "5", "--body", "I ran the demo"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let t = tick(&h);
    assert!(
        t.stdout.contains(
            "unattended coordinator: the verdicts of today are used (1/1); told the owner: "
        ),
        "{}",
        t.all()
    );
    let notes = spent_notes(&h);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("to=owner"), "{}", notes[0]);
    assert!(notes[0].contains("kind=info"), "{}", notes[0]);
    assert!(
        notes[0].contains("The unattended coordinator used its verdicts of "),
        "{}",
        notes[0]
    );
    // the note is no stamped message: the counts are as the verdict left them
    assert_eq!(stamps_in(&notes[0]), Vec::<&str>::new());
    assert_eq!(coord_lines(&t)[0], count_text("verify", 1, 1));
    // another tick of the day: no note and no line
    let t = tick(&h);
    assert_eq!(spent_notes(&h).len(), 1);
    assert_eq!(coord_lines(&t), [count_text("verify", 1, 1)], "{}", t.all());
    // a host that lost its state finds the note in the registry issue
    std::fs::remove_file(state_path(&h)).unwrap();
    let t = tick(&h);
    assert_eq!(spent_notes(&h).len(), 1, "{}", t.all());
    assert_eq!(coord_lines(&t), [count_text("verify", 1, 1)], "{}", t.all());
    assert_eq!(
        StateCache::load(&state_path(&h)).tick.unattended_spent_told,
        Some(crewboard::clock::format_ts(now_epoch())[..10].to_string())
    );
}

#[test]
fn a_dev_agent_that_posts_the_key_does_not_keep_the_note_from_the_owner() {
    settle_day();
    let h = hub();
    h.set_authority_extra("unattended_coordinator: verify\nunattended_budget: 1\n");
    let day = crewboard::clock::format_ts(now_epoch())[..10].to_string();
    h.board.signed_now(
        &h.dev.key,
        &h.dev.name,
        "owner",
        Kind::Info,
        REGISTRY,
        &format!("told already\nevent=unattended-budget:{day}\n"),
    );
    let o = h.headless_read(&["verify", "5", "--body", "I ran the demo"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let t = tick(&h);
    assert!(t.stdout.contains("told the owner: "), "{}", t.all());
    let notes: Vec<String> = spent_notes(&h)
        .into_iter()
        .filter(|b| b.contains("The unattended coordinator used its verdicts of "))
        .collect();
    assert_eq!(notes.len(), 1, "{notes:?}");
}

#[test]
fn a_line_that_only_crewboard_writes_is_refused_in_every_config_dir() {
    const RULE: &str = "[rule=text-reads-as-act]";
    let h = hub();
    h.set_authority_extra(VERIFY);
    let dev_name = h.dev.name.clone();
    for text in [
        "fine\nunattended: 9/10",
        "fine\nverified-by: unattended",
        "  verified-by: me",
    ] {
        for argv in [
            vec!["verify", "5", "--body", text],
            vec![
                "say", "5", "--to", &dev_name, "--kind", "answer", "--body", text,
            ],
            vec!["ask", "5", "--body", text],
            vec!["unblock", "5", "--body", text],
            vec!["close", "5", "--why", text],
        ] {
            // the coordinator, in its interactive dir and in the headless one
            for o in [h.interactive(&argv), h.headless(&argv)] {
                assert_eq!(o.code, 2, "{argv:?}: {}", o.all());
                // `close --why` is outside the headless scope before any text is read
                let scope = argv[0] == "close" && o.stderr.contains(SCOPE);
                assert!(scope || o.stderr.contains(RULE), "{argv:?}: {}", o.all());
                assert!(
                    scope || o.stderr.contains("only crewboard writes that line"),
                    "{argv:?}: {}",
                    o.all()
                );
            }
        }
        // `block --why` of a dev agent, in its interactive dir
        let o = h.run(&h.dev.dir, &["block", "5", "--why", text]);
        refused(&o, RULE);
        assert!(
            o.stderr.contains("only crewboard writes that line"),
            "{}",
            o.all()
        );
        // `block` is outside the scope of a headless dir, with any text
        let o = h.headless(&["block", "5", "--why", text]);
        refused(&o, SCOPE);
    }
    assert!(h.writes().is_empty(), "{:?}", h.writes().len());
    // the merge notice that crewboard writes itself is posted
    stamped_pass(&h);
    let o = h.interactive(&["merge", "5"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(is_verified_by_unattended(&last_comment(&h)));
}

/// The seconds since a time of the UTC day before today, one hour before its end.
fn yesterday() -> i64 {
    now_epoch().rem_euclid(86_400) + 3_600
}

/// The body of the one project status update of the hub.
fn digest_body(h: &Hub) -> String {
    let ups = h.board.status_updates();
    assert_eq!(ups.len(), 1, "{ups:?}");
    ups[0].2.clone()
}

/// The reads of the comment feed from the start of the UTC day before today.
fn yesterday_feed_reads(h: &Hub) -> usize {
    let start = now_epoch() - now_epoch().rem_euclid(86_400) - 86_400;
    let since = format!("since={}", crewboard::clock::format_ts(start));
    h.requests()
        .iter()
        .filter(|r| r.method == "GET" && r.path.contains(&since))
        .count()
}

#[test]
fn the_daily_digest_has_a_part_about_the_unattended_coordinator_with_facts_only() {
    settle_day();
    let h = hub();
    h.set_authority_extra(&format!("{VERIFY}daily_digest: true\n"));
    let ago = yesterday();
    let stamp = |n: u32| format!("{}\n", unattended_line(n, 10));
    // the day before: a pass and a fail on #5, an answer, a question to the owner
    let d = latest_done(&h, TICKET);
    h.board.signed_re_ago(
        &h.coord.key,
        &h.coord.name,
        &h.dev.name,
        Kind::Report,
        TICKET,
        d,
        &format!(
            "verify: fail\nsha: {SHA}\nfailed: the demo does not run\n{}",
            stamp(1)
        ),
        ago + 60,
    );
    h.board.signed_ago(
        &h.coord.key,
        &h.coord.name,
        &h.dev.name,
        Kind::Answer,
        TICKET,
        &format!("SECRET-TEXT-OF-A-MESSAGE\n{}", stamp(1)),
        ago + 50,
    );
    h.board.signed_ago(
        &h.dev.key,
        &h.dev.name,
        &h.coord.name,
        Kind::Done,
        TICKET,
        &format!(
            "Done. Evidence follows.\npr: https://github.com/{REPO}/pull/{PR}\nsha: {SHA}\nci: https://github.com/{REPO}/actions/runs/9002\nround: 2\n"
        ),
        ago + 40,
    );
    let d2 = latest_done(&h, TICKET);
    pass_ago(&h, TICKET, d2, &stamp(2), ago + 30);
    let t = tick(&h);
    assert!(t.stdout.contains("digest: posted"), "{}", t.all());
    let body = format!("{}\n", digest_body(&h).trim_end());
    let day = crewboard::clock::format_ts(now_epoch() - ago)[..10].to_string();
    let k = key(TICKET);
    assert!(
        body.ends_with(&format!(
            "\nUnattended coordinator (mode verify), messages of {day} (UTC):\n- verdicts: pass 1 ({k}); fail 1 ({k}); budget used 2/10\n- answers 1; assignments 0; questions to the owner 0\n- merged after a pass of the unattended coordinator: 0\nVerified by the unattended coordinator, in Review and not merged: 1 ({k})\n"
        )),
        "{body}"
    );
    // no text of a message
    assert!(!body.contains("SECRET-TEXT"), "{body}");
    assert!(!body.contains("the demo does not run"), "{body}");
    assert_eq!(yesterday_feed_reads(&h), 1);
    // the same day: no second read
    tick(&h);
    assert_eq!(yesterday_feed_reads(&h), 1);
}

#[test]
fn the_digest_names_a_merge_after_an_unattended_pass() {
    settle_day();
    let h = hub();
    h.set_authority_extra(&format!("{MERGE}daily_digest: true\n"));
    let ago = yesterday();
    h.board.signed_ago(
        &h.coord.key,
        &h.coord.name,
        &h.dev.name,
        Kind::Info,
        9,
        &format!(
            "{NOTICE}\npr: https://github.com/{REPO}/pull/3\nsha: {SHA}\nmethod: squash\nmerge-commit: abc\nbranch: b\nverified-by: unattended"
        ),
        ago,
    );
    // a dev agent writes the same text: it is no merge notice of a coordinator
    h.board.signed_ago(
        &h.dev.key,
        &h.dev.name,
        &h.coord.name,
        Kind::Info,
        10,
        &format!("{NOTICE}\npr: p\nverified-by: unattended"),
        ago,
    );
    tick(&h);
    let body = digest_body(&h);
    assert!(
        body.contains(&format!(
            "- merged after a pass of the unattended coordinator: 1 ({})\n",
            key(9)
        )),
        "{body}"
    );
    assert!(
        body.contains("Unattended coordinator (mode verify-and-merge), "),
        "{body}"
    );
}

#[test]
fn with_the_key_off_the_digest_is_the_one_without_the_key_and_reads_no_feed() {
    let mut bodies = Vec::new();
    for extra in [
        "daily_digest: true\n",
        "unattended_coordinator: off\ndaily_digest: true\n",
    ] {
        let h = hub();
        h.set_authority_extra(extra);
        // a stamped message of the day before, which the state of this host does not know
        let d = latest_done(&h, TICKET);
        pass_ago(
            &h,
            TICKET,
            d,
            &format!("{}\n", unattended_line(1, 10)),
            yesterday(),
        );
        tick(&h);
        let body = digest_body(&h);
        assert!(!body.contains("Unattended"), "{body}");
        assert!(!body.contains("unattended"), "{body}");
        assert_eq!(yesterday_feed_reads(&h), 0, "{extra:?}");
        bodies.push(body);
    }
    assert_eq!(bodies[0], bodies[1]);
    // the key is off today, and the state holds stamped messages of the day before: the part
    // is there, so the owner reads what happened before the switch
    let h = hub();
    h.set_authority_extra("daily_digest: true\n");
    let ago = yesterday();
    let day = crewboard::clock::format_ts(now_epoch() - ago)[..10].to_string();
    let d = latest_done(&h, TICKET);
    pass_ago(&h, TICKET, d, &format!("{}\n", unattended_line(1, 10)), ago);
    let mut state = StateCache::load(&state_path(&h));
    state.tick.unattended_messages_floor = Some((day.clone(), 1));
    std::fs::create_dir_all(state_path(&h).parent().unwrap()).unwrap();
    state.save(&state_path(&h)).unwrap();
    tick(&h);
    let body = digest_body(&h);
    assert!(
        body.contains(&format!(
            "Unattended coordinator (mode off), messages of {day} (UTC):\n- verdicts: pass 1 ({}); fail 0; budget used 1/10\n",
            key(TICKET)
        )),
        "{body}"
    );
}
