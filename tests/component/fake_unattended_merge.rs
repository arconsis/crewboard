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
    // the next tick merges: no model takes part
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
            .contains("no passing verify result of a coordinator"),
        "{}",
        o.all()
    );
    assert!(h.writes().is_empty());
    stamped_pass(&h);
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
        let t = tick(&h);
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
    let t = tick(&h);
    assert!(!h.merged(), "{}", t.all());
    assert_eq!(merge_calls(&h), 0);
}

#[test]
fn an_attended_pass_counts_in_the_merge_mode_and_its_notice_has_no_line() {
    let h = hub();
    h.set_authority_extra(MERGE);
    // the interactive coordinator verifies and leaves the merge
    let o = h.interactive(&["verify", "5", "--body", "I ran the demo"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(stamps_in(&last_comment(&h)), Vec::<&str>::new());
    let t = tick(&h);
    assert!(h.merged(), "{}", t.all());
    let notice = last_comment(&h);
    assert!(notice.contains(NOTICE), "{notice}");
    assert!(!is_verified_by_unattended(&notice), "{notice}");
    // in the mode `verify` the same pass merges nothing
    let v = hub();
    v.set_authority_extra(VERIFY);
    let o = v.interactive(&["verify", "5", "--body", "I ran the demo"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let t = tick(&v);
    assert!(!v.merged(), "{}", t.all());
    // and a person who merges after a pass of the unattended coordinator gets the line
    let p = hub();
    p.set_authority_extra(VERIFY);
    stamped_pass(&p);
    let o = p.interactive(&["merge", "5"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(is_verified_by_unattended(&last_comment(&p)));
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
            refusal: "no passing verify result of a coordinator",
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
            refusal: "no passing verify result of a coordinator",
        },
        Case {
            name: "a later done",
            scenario: plain,
            before: nothing,
            after: |h| {
                done(h, TICKET, "round: 2\n");
            },
            refusal: "no passing verify result of a coordinator",
        },
        Case {
            name: "a failed result after the pass",
            scenario: plain,
            before: nothing,
            after: |h| {
                let d = latest_done(h, TICKET);
                verdict(h, TICKET, d, "");
            },
            refusal: "no passing verify result of a coordinator",
        },
    ]
}

#[test]
fn every_other_refusal_of_the_merge_still_fires_in_the_merge_mode() {
    // control: the setup of these cases merges when no reason is added
    let ok = Hub::new(plain());
    ok.set_authority_extra(MERGE);
    stamped_pass(&ok);
    let t = tick(&ok);
    assert!(ok.merged(), "{}", t.all());

    for c in cases() {
        // the command `merge`, and the tick, each on a board of its own
        for by_tick in [false, true] {
            let h = Hub::new((c.scenario)());
            h.set_authority_extra(MERGE);
            (c.before)(&h);
            stamped_pass(&h);
            (c.after)(&h);
            if by_tick {
                let t = tick(&h);
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
    let t = tick(&h);
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
    let t = tick(&h);
    assert!(!h.merged(), "{}", t.all());

    // a line with the key that is not valid is no stamp and no attended result
    let h = hub();
    h.set_authority_extra(MERGE);
    let d = latest_done(&h, TICKET);
    pass_ago(&h, TICKET, d, "unattended: many\n", 0);
    refused(&h.headless_read(&["merge", "5"]), SCOPE);
    let t = tick(&h);
    assert!(!h.merged(), "{}", t.all());

    // an old pass does not count for a new done, and the new verdict does
    let h = hub();
    h.set_authority_extra(MERGE);
    stamped_pass(&h);
    done(&h, TICKET, "round: 2\n");
    let t = tick(&h);
    assert!(!h.merged(), "{}", t.all());
    stamped_pass(&h);
    let t = tick(&h);
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
    pass_ago(&h, TICKET, d, "", 0);
    let o = h.headless_read(&["merge", "5"]);
    refused(&o, SCOPE);
    assert!(o.stderr.contains("it is not CI only"), "{}", o.all());
    let t = tick(&h);
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
