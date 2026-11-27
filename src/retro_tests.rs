use super::*;
use crate::protocol::acts::{EvidenceMsg, VerifyFailure, done_body, verify_body};
use crate::testkit::agent;

const COORD: &str = "agent-a@host-a";
const DEV: &str = "agent-b@host-b";
const OTHER_DEV: &str = "agent-c@host-c";
const HOUR: i64 = 3600;

/// The report without the commits of the default branch: the reverts are not read.
fn retro(logs: &[TicketLog]) -> Report {
    super::retro(logs, None)
}

/// The messages of one issue under construction: the comment ids count up, and each message is
/// created at the hour that the caller names.
struct Log {
    issue: u64,
    msgs: Vec<VMsg>,
}

impl Log {
    fn new(issue: u64) -> Log {
        Log {
            issue,
            msgs: Vec::new(),
        }
    }

    fn push(mut self, hour: i64, from: &str, role: Role, to: &str, kind: Kind, body: &str) -> Log {
        let id = self.issue * 100 + self.msgs.len() as u64;
        self.msgs.push(VMsg {
            created: hour * HOUR,
            ..crate::testkit::msg(id, agent(from, role), to, Some(kind), body)
        });
        self
    }

    fn coord(self, hour: i64, kind: Kind, body: &str) -> Log {
        self.push(hour, COORD, Role::Coordinator, DEV, kind, body)
    }

    fn dev(self, hour: i64, to: &str, kind: Kind, body: &str) -> Log {
        self.push(hour, DEV, Role::Dev, to, kind, body)
    }

    fn owner(mut self, hour: i64, body: &str) -> Log {
        let id = self.issue * 100 + self.msgs.len() as u64;
        self.msgs.push(VMsg {
            created: hour * HOUR,
            ..crate::testkit::msg(id, Sender::Owner, "coordinator", None, body)
        });
        self
    }

    fn assign(self, hour: i64) -> Log {
        self.coord(hour, Kind::Request, "Assigned to you: t\ngate: g\n")
    }

    fn done(self, hour: i64) -> Log {
        let body = done_body(&evidence(), "");
        self.dev(hour, "coordinator", Kind::Done, &body)
    }

    fn pass(self, hour: i64) -> Log {
        let body = verify_body(true, &evidence(), "abc", &[], "", None, None);
        self.coord(hour, Kind::Report, &body)
    }

    /// A pass whose local-run line the tool wrote.
    fn tool_pass(self, hour: i64) -> Log {
        let line = "tool-run host-a: tests rc=0 5s; clean checkout of abc";
        let body = verify_body(true, &evidence(), "abc", &[], "", None, Some(line));
        self.coord(hour, Kind::Report, &body)
    }

    /// A pass whose local-run line a model typed.
    fn typed_pass(self, hour: i64) -> Log {
        let line = "host-a: make test rc=0, log /tmp/x.log";
        let body = verify_body(true, &evidence(), "abc", &[], "", None, Some(line));
        self.coord(hour, Kind::Report, &body)
    }

    fn fail(self, hour: i64, failures: &[VerifyFailure]) -> Log {
        let body = verify_body(false, &evidence(), "def", failures, "", None, None);
        self.coord(hour, Kind::Report, &body)
    }

    fn merged(self, hour: i64) -> Log {
        self.coord(hour, Kind::Info, "Merged by the coordinator.\n")
    }

    fn build(self) -> TicketLog {
        TicketLog {
            ticket: TicketRef::new("a/b", self.issue),
            urls: self
                .msgs
                .iter()
                .map(|m| (m.comment_id, url(m.comment_id)))
                .collect(),
            msgs: self.msgs,
            outside_window: 0,
        }
    }
}

fn url(comment: u64) -> String {
    format!("https://github.com/a/b/issues/1#issuecomment-{comment}")
}

fn evidence() -> EvidenceMsg {
    EvidenceMsg {
        pr: "https://github.com/a/b/pull/7".into(),
        sha: "abc".into(),
        ci: "none".into(),
        ci_sources: None,
        ci_runs: None,
        local_gate: None,
    }
}

fn head_moved() -> VerifyFailure {
    VerifyFailure::HeadMoved {
        evidence_sha: "abc".into(),
        head_now: "def".into(),
    }
}

fn review(note: &str) -> VerifyFailure {
    VerifyFailure::GateCheck { note: note.into() }
}

fn red() -> VerifyFailure {
    VerifyFailure::CiNotGreen {
        sha: "abc".into(),
        red: vec!["build".into()],
    }
}

fn class<'a>(r: &'a Report, word: &str) -> Vec<&'a Event> {
    r.classes
        .iter()
        .filter(|c| c.class == word)
        .flat_map(|c| &c.events)
        .collect()
}

fn issues(events: &[&Event]) -> Vec<u64> {
    events.iter().map(|e| e.ticket.issue).collect()
}

#[test]
fn an_empty_period_and_an_issue_without_an_assignment_give_an_empty_report() {
    assert_eq!(retro(&[]), Report::default());
    // a question on an issue that no coordinator assigned is no ticket of the report
    let log = Log::new(1)
        .dev(1, "owner", Kind::Question, "which API?")
        .build();
    assert_eq!(retro(&[log]), Report::default());
    // an assignment text of a dev agent assigns nothing
    let log = Log::new(1)
        .dev(1, DEV, Kind::Request, "Assigned to you: t\ngate: g\n")
        .done(2)
        .build();
    assert_eq!(retro(&[log]).tickets, 0);
}

#[test]
fn volume_and_period_count_the_messages_of_tickets_only() {
    let a = Log::new(1).assign(10).done(12).pass(13).merged(14).build();
    let b = Log::new(2).assign(11).build();
    let no_ticket = Log::new(3).dev(1, "owner", Kind::Question, "early").build();
    let r = retro(&[a, b, no_ticket]);
    assert_eq!((r.tickets, r.delivered, r.messages), (2, 1, 5));
    assert_eq!(r.outside_window, 0);
    // the messages that fail only the replay window are counted for the tickets
    let late = TicketLog {
        outside_window: 2,
        ..Log::new(4).assign(1).build()
    };
    let no_ticket = TicketLog {
        outside_window: 1,
        ..Log::new(5).dev(1, "owner", Kind::Question, "q").build()
    };
    assert_eq!(retro(&[late, no_ticket]).outside_window, 2);
    assert_eq!((r.first, r.last), (Some(10 * HOUR), Some(14 * HOUR)));
}

#[test]
fn a_done_counts_only_from_the_assignee_and_with_evidence() {
    let body = done_body(&evidence(), "");
    let log = Log::new(1)
        .assign(1)
        // another dev agent, and the assignee without evidence
        .push(2, OTHER_DEV, Role::Dev, "coordinator", Kind::Done, &body)
        .dev(3, "coordinator", Kind::Done, "Done, trust me.")
        .build();
    assert_eq!(retro(&[log]).delivered, 0);
    // after a reassign the old assignee delivers nothing
    let log = Log::new(1)
        .assign(1)
        .coord(2, Kind::Info, "Reassigned. Reason: the agent is late")
        .done(3)
        .build();
    let r = retro(&[log]);
    assert_eq!((r.delivered, r.flow.reassigned_tickets), (0, 1));
}

#[test]
fn a_done_after_the_close_counts_only_when_the_ticket_is_open_again() {
    let merged = || Log::new(1).assign(0).done(2).pass(3).merged(4);
    let spread_of = |log: Log| retro(&[log.build()]).flow.assign_to_done;
    // control: one done, two hours after the assignment
    let one = spread_of(merged()).unwrap();
    // a done of the old assignee after the close changes nothing
    let late_done = Log::new(2).assign(0).merged(4).done(9);
    assert_eq!(retro(&[late_done.build()]).delivered, 0);
    assert_eq!(spread_of(merged().done(9)), Some(one));
    // the owner moves the ticket back: the assignee delivers again
    let moved = Log::new(3)
        .assign(0)
        .merged(4)
        .owner(5, "/crew move Ready")
        .done(9);
    assert_eq!(retro(&[moved.build()]).delivered, 1);
    // a plain owner comment opens nothing
    let talk = Log::new(4).assign(0).merged(4).owner(5, "thanks").done(9);
    assert_eq!(retro(&[talk.build()]).delivered, 0);
    // a new assignment opens it too
    let again = Log::new(5).assign(0).merged(4).assign(6).done(9);
    assert_eq!(retro(&[again.build()]).delivered, 1);
}

#[test]
fn verify_counts_the_first_result_and_the_fails() {
    let first_pass = Log::new(1).assign(1).done(2).pass(3).merged(4).build();
    let one_round = Log::new(2)
        .assign(1)
        .done(2)
        .fail(3, &[review("part 2 is missing")])
        .done(4)
        .pass(5)
        .merged(6)
        .build();
    let two_rounds = Log::new(3)
        .assign(1)
        .done(2)
        .fail(3, &[red()])
        .done(4)
        .fail(5, &[review("still wrong")])
        .build();
    let not_verified = Log::new(4).assign(1).done(2).build();
    let r = retro(&[first_pass, one_round, two_rounds, not_verified]);
    assert_eq!(
        r.verify,
        VerifyStats {
            verified: 3,
            first_pass: 1,
            first_pass_or_accepted: 1,
            tickets_with_fail: 2,
            fails: 3,
            rework_tickets: 2,
            rework_fails: 3,
            repeat_rework_tickets: 1,
            passes: 2,
            tool_run_passes: 0,
        }
    );
    // a verify text of a dev agent is no verify result
    let forged = Log::new(5)
        .assign(1)
        .done(2)
        .dev(3, "coordinator", Kind::Report, "verify: fail\nfailed: x\n")
        .build();
    assert_eq!(retro(&[forged]).verify, VerifyStats::default());
}

#[test]
fn each_failure_of_a_fail_gives_its_class_and_a_fail_with_two_classes_is_in_both_rows() {
    let all: Vec<(VerifyFailure, &str)> = vec![
        (review("wrong"), "review"),
        (head_moved(), "head-moved"),
        (red(), "ci-red"),
        (
            VerifyFailure::NoCiRun {
                sha: "abc".into(),
                not_accepted: None,
                cause: "c".into(),
            },
            "ci-missing",
        ),
        (
            VerifyFailure::NoCountedCiRun {
                sha: "abc".into(),
                info: vec!["9 (schedule)".into()],
                cause: "c".into(),
            },
            "ci-missing",
        ),
        (
            VerifyFailure::CiFinding {
                message: "run 9 is gone".into(),
                rule: "evidence-ci-missing".into(),
            },
            "ci-evidence",
        ),
        (
            VerifyFailure::CiUnreadable {
                why: "HTTP 403".into(),
            },
            "ci-unreadable",
        ),
        (VerifyFailure::CiNoneUndeclared, "ci-none-undeclared"),
        (VerifyFailure::LocalGateMissing, "local-gate"),
        (VerifyFailure::LocalGateNotEnabled, "local-gate"),
        (VerifyFailure::PrClosed, "pr-closed"),
        (
            VerifyFailure::PrOutsideInitiative {
                repo: "a/c".into(),
                number: 7,
            },
            "pr-repo",
        ),
    ];
    for (n, (failure, word)) in all.iter().enumerate() {
        let issue = n as u64 + 1;
        let log = Log::new(issue)
            .assign(1)
            .done(2)
            .fail(3, std::slice::from_ref(failure))
            .build();
        let r = retro(&[log]);
        assert_eq!(r.classes.len(), 1, "{failure:?}");
        assert_eq!(r.classes[0].class, *word, "{failure:?}");
        let e = &r.classes[0].events[0];
        assert_eq!((e.ticket.issue, e.to.as_str()), (issue, DEV));
        assert_eq!(e.url, url(issue * 100 + 2));
    }
    // two classes in one fail: one fail, two rows
    let log = Log::new(1)
        .assign(1)
        .done(2)
        .fail(3, &[red(), review("and the doc is wrong")])
        .build();
    let r = retro(&[log]);
    assert_eq!(r.verify.fails, 1);
    let words: Vec<&str> = r.classes.iter().map(|c| c.class).collect();
    assert_eq!(words, ["review", "ci-red"]);
    // a fail of an older build without a `failed:` line is the coordinator's text; a line that
    // no class knows is `other`
    let log = Log::new(2)
        .assign(1)
        .done(2)
        .coord(3, Kind::Report, "verify: fail\nnote: the doc is wrong\n")
        .done(4)
        .coord(
            5,
            Kind::Report,
            "verify: fail\nfailed: a text of a newer build\n",
        )
        .build();
    let r = retro(&[log]);
    assert_eq!(issues(&class(&r, "review")), [2]);
    assert_eq!(issues(&class(&r, "other")), [2]);
}

/// The verdict of the artifact rule for the fails of one ticket, in order: `true` for a fail
/// that is `accepted-after-head-moved`.
fn accepted(log: Log) -> Vec<bool> {
    let r = retro(&[log.build()]);
    let ok: Vec<u64> = class(&r, ACCEPTED_AFTER_HEAD_MOVED)
        .iter()
        .map(|e| e.comment_id)
        .collect();
    let mut all: Vec<u64> = r
        .classes
        .iter()
        .flat_map(|c| &c.events)
        .map(|e| e.comment_id)
        .collect();
    all.sort_unstable();
    all.dedup();
    // an accepted fail is in no other class row
    for c in r
        .classes
        .iter()
        .filter(|c| c.class != ACCEPTED_AFTER_HEAD_MOVED)
    {
        assert!(c.events.iter().all(|e| !ok.contains(&e.comment_id)));
    }
    assert_eq!(r.verify.fails, all.len());
    assert_eq!(r.verify.rework_fails, all.len() - ok.len());
    all.iter().map(|id| ok.contains(id)).collect()
}

const OWNER_MERGED: &str = "The ticket is closed: the owner merged the PR by hand.\n";
const VERIFIED: &str = "Closed after a passed verify\n";

fn start() -> Log {
    Log::new(1).assign(1).done(2)
}

#[test]
fn a_head_moved_fail_is_accepted_when_a_pass_or_a_merge_close_comes_next() {
    // (b) the next verify is a pass: with a new `done` in between, and without one
    assert_eq!(
        accepted(start().fail(3, &[head_moved()]).done(4).pass(5).merged(6)),
        [true]
    );
    assert_eq!(accepted(start().fail(3, &[head_moved()]).pass(4)), [true]);
    // (b) the ticket closes as merged, by each close that says so, with or without a `done`
    for close in ["Merged by the coordinator.\n", OWNER_MERGED, VERIFIED] {
        let log = start().fail(3, &[head_moved()]).coord(4, Kind::Info, close);
        assert_eq!(accepted(log), [true], "{close}");
        let log = start()
            .fail(3, &[head_moved()])
            .done(4)
            .coord(5, Kind::Info, close);
        assert_eq!(accepted(log), [true], "{close}");
    }
}

#[test]
fn a_head_moved_fail_with_a_second_class_stays_rework() {
    // (a) a review text in the same message
    assert_eq!(
        accepted(
            start()
                .fail(3, &[head_moved(), review("the doc is wrong")])
                .done(4)
                .pass(5)
        ),
        [false]
    );
    // (a) another machine class in the same message
    assert_eq!(
        accepted(start().fail(3, &[head_moved(), red()]).done(4).pass(5)),
        [false]
    );
    // a fail of another class alone is never accepted
    assert_eq!(accepted(start().fail(3, &[red()]).done(4).pass(5)), [false]);
    assert_eq!(
        accepted(start().fail(3, &[review("wrong")]).merged(4)),
        [false]
    );
}

#[test]
fn a_head_moved_fail_is_rework_when_a_fail_a_drop_or_nothing_comes_next() {
    // the ticket is still open
    assert_eq!(accepted(start().fail(3, &[head_moved()])), [false]);
    assert_eq!(accepted(start().fail(3, &[head_moved()]).done(4)), [false]);
    // dropped, or closed by the judgment of an older build: no merge
    for close in ["dropped: not needed\n", "closed: not needed\n"] {
        let log = start().fail(3, &[head_moved()]).coord(4, Kind::Info, close);
        assert_eq!(accepted(log), [false], "{close}");
    }
    // another fail in between: the first fail is rework, whatever the class of the second is.
    // The second one is judged by what follows it.
    assert_eq!(
        accepted(
            start()
                .fail(3, &[head_moved()])
                .done(4)
                .fail(5, &[review("wrong")])
                .done(6)
                .pass(7)
        ),
        [false, false]
    );
    assert_eq!(
        accepted(
            start()
                .fail(3, &[head_moved()])
                .fail(4, &[head_moved()])
                .done(5)
                .pass(6)
        ),
        [false, true]
    );
    // a review fail, then an accepted head-moved fail
    assert_eq!(
        accepted(
            start()
                .fail(3, &[review("wrong")])
                .done(4)
                .fail(5, &[head_moved()])
                .coord(6, Kind::Info, OWNER_MERGED)
        ),
        [false, true]
    );
    // a verify text of a dev agent is no pass
    assert_eq!(
        accepted(start().fail(3, &[head_moved()]).dev(
            4,
            "coordinator",
            Kind::Report,
            "verify: pass\n"
        )),
        [false]
    );
}

#[test]
fn the_rework_numbers_and_the_first_verify_leave_the_accepted_fails_out() {
    // first verify: an accepted fail
    let a = Log::new(1)
        .assign(1)
        .done(2)
        .fail(3, &[head_moved()])
        .done(4)
        .pass(5)
        .merged(6)
        .build();
    // first verify: a pass
    let b = Log::new(2).assign(1).done(2).pass(3).merged(4).build();
    // first verify: rework, then an accepted fail
    let c = Log::new(3)
        .assign(1)
        .done(2)
        .fail(3, &[review("wrong")])
        .done(4)
        .fail(5, &[head_moved()])
        .done(6)
        .pass(7)
        .build();
    // first verify: a head-moved fail that is rework (the ticket is open)
    let d = Log::new(4)
        .assign(1)
        .done(2)
        .fail(3, &[head_moved()])
        .build();
    let r = retro(&[a, b, c, d]);
    assert_eq!(
        r.verify,
        VerifyStats {
            verified: 4,
            first_pass: 1,
            first_pass_or_accepted: 2,
            tickets_with_fail: 3,
            fails: 4,
            rework_tickets: 2,
            rework_fails: 2,
            repeat_rework_tickets: 0,
            passes: 3,
            tool_run_passes: 0,
        }
    );
    assert_eq!(issues(&class(&r, ACCEPTED_AFTER_HEAD_MOVED)), [1, 3]);
    assert_eq!(issues(&class(&r, "head-moved")), [4]);
    assert_eq!(issues(&class(&r, "review")), [3]);
    // the rounds leave the accepted fails out
    let got: Vec<(u64, usize)> = r.rounds.iter().map(|x| (x.ticket.issue, x.fails)).collect();
    assert_eq!(got, [(3, 1), (4, 1)]);
}

#[test]
fn the_rounds_are_the_five_tickets_with_the_most_fails() {
    let mut logs = Vec::new();
    for issue in 1..=7u64 {
        let mut log = Log::new(issue).assign(1).done(2);
        // ticket n has n - 1 fails
        for round in 0..issue as i64 - 1 {
            log = log
                .fail(3 + 2 * round, &[review("again")])
                .done(4 + 2 * round);
        }
        logs.push(log.build());
    }
    let r = retro(&logs);
    let got: Vec<(u64, usize)> = r.rounds.iter().map(|x| (x.ticket.issue, x.fails)).collect();
    assert_eq!(got, [(7, 6), (6, 5), (5, 4), (4, 3), (3, 2)]);
    // the link is the latest fail
    assert_eq!(r.rounds[4].latest.url, url(3 * 100 + 4));
}

#[test]
fn andon_counts_questions_blocks_and_parks_and_lists_the_open_question() {
    let answered = Log::new(1)
        .assign(1)
        .dev(2, "owner", Kind::Question, "which API?")
        .owner(3, "the new one")
        .dev(4, "coordinator", Kind::Question, "which branch?")
        .dev(5, COORD, Kind::Question, "and the base?")
        .dev(6, "coordinator", Kind::Blocked, "the runner is down")
        .dev(7, "coordinator", Kind::Blocked, "parked for #9: urgent")
        .build();
    let open = Log::new(2)
        .assign(1)
        .dev(2, "owner", Kind::Question, "may I drop the cache?")
        .build();
    // an open question on a ticket that has ended is not listed
    let ended = Log::new(3)
        .assign(1)
        .dev(2, "owner", Kind::Question, "too late?")
        .coord(3, Kind::Info, "dropped: not needed\n")
        .build();
    // a park text counts as a park only from the assignee of a ticket that has not ended:
    // from another dev agent, from the coordinator and after the close it is a plain block
    let park = "parked for #9: urgent";
    let look_alike = Log::new(4)
        .assign(1)
        .push(2, OTHER_DEV, Role::Dev, "coordinator", Kind::Blocked, park)
        .coord(3, Kind::Blocked, park)
        .merged(4)
        .dev(5, "coordinator", Kind::Blocked, park)
        .build();
    let r = retro(&[look_alike]);
    assert_eq!((r.andon.parked.len(), r.andon.blocked.len()), (0, 3));
    let r = retro(&[answered, open, ended]);
    let a = &r.andon;
    assert_eq!(
        (
            a.questions_owner.len(),
            a.questions_coordinator.len(),
            a.questions_other.len(),
            a.blocked.len(),
            a.parked.len()
        ),
        (3, 1, 1, 1, 1)
    );
    assert_eq!(a.open_questions.len(), 1);
    assert_eq!(a.open_questions[0].ticket.issue, 2);
    assert_eq!(a.open_questions[0].url, url(201));
}

#[test]
fn a_round_limit_stop_is_a_coordinator_question_with_the_fixed_first_line() {
    let text = crate::protocol::acts::round_limit_question(3, 3, &url(104));
    let stopped = Log::new(1)
        .assign(1)
        .done(2)
        .fail(3, &[review("again")])
        .coord(4, Kind::Question, "placeholder")
        .build();
    // `coord` sends to the dev agent: a question to another addressee is no stop
    assert!(retro(&[stopped]).andon.round_limit_stops.is_empty());
    let stopped = Log::new(1)
        .assign(1)
        .done(2)
        .fail(3, &[review("again")])
        .push(4, COORD, Role::Coordinator, "owner", Kind::Question, &text)
        .owner(5, "Split it.")
        .push(6, COORD, Role::Coordinator, "owner", Kind::Question, &text)
        .build();
    // the same text of a dev agent, and another question of the coordinator, are no stop
    let other = Log::new(2)
        .assign(1)
        .dev(2, "owner", Kind::Question, &text)
        .push(
            3,
            COORD,
            Role::Coordinator,
            "owner",
            Kind::Question,
            "Which API?",
        )
        .build();
    let r = retro(&[stopped, other]);
    let stops: Vec<String> = r
        .andon
        .round_limit_stops
        .iter()
        .map(|e| e.url.clone())
        .collect();
    assert_eq!(stops, [url(103), url(105)]);
    // every stop is a question to the owner, too
    assert_eq!(r.andon.questions_owner.len(), 4);
}

#[test]
fn a_refusal_id_counts_as_a_whole_word_in_a_question_a_block_or_a_report_of_a_dev_agent() {
    let known = ids::VERIFY_NO_EVIDENCE.as_str();
    assert_eq!(
        refusals_mentioned(&format!("got [{known}] twice: {known}")),
        [known]
    );
    let other = ids::VERIFY_FAIL_NEEDS_NOTE.as_str();
    let mut both = refusals_mentioned(&format!("refused: {other}, then `{known}`."));
    both.sort_unstable();
    let mut want = [known, other];
    want.sort_unstable();
    assert_eq!(both, want);
    // part of a longer word is no mention
    for text in [
        format!("x{known}"),
        format!("{known}-again"),
        format!("{known}_x"),
        format!("pre-{known}"),
        "no id here".to_string(),
    ] {
        assert!(refusals_mentioned(&text).is_empty(), "{text}");
    }
    let log = Log::new(1)
        .assign(1)
        .dev(
            2,
            "coordinator",
            Kind::Blocked,
            &format!("refused with {known}"),
        )
        .dev(3, "owner", Kind::Question, &format!("{known} again, why?"))
        .dev(
            4,
            "coordinator",
            Kind::Report,
            &format!("{other} and {known}"),
        )
        // not counted: the coordinator, and a kind that is no question, block or report
        .coord(5, Kind::Report, &format!("I saw {known}"))
        .dev(6, "coordinator", Kind::Ack, known)
        .build();
    let r = retro(&[log]);
    let rows: Vec<(&str, usize)> = r
        .refusals
        .iter()
        .map(|x| (x.rule, x.events.len()))
        .collect();
    assert_eq!(rows, [(known, 3), (other, 1)]);
    assert_eq!(r.refusals[0].events[0].url, url(101));
    assert_eq!(r.refusals[0].events[0].from, DEV);
}

#[test]
fn flow_counts_reassigns_drops_closes_and_late_notes() {
    let merged = Log::new(1).assign(1).done(2).pass(3).merged(4).build();
    let verified = Log::new(2)
        .assign(1)
        .done(2)
        .pass(3)
        .coord(4, Kind::Info, "Closed after a passed verify\n")
        .build();
    let dropped = Log::new(3)
        .assign(1)
        .coord(2, Kind::Info, "Reassigned. Reason: the agent is late")
        .assign(3)
        .coord(4, Kind::Info, "Reassigned. Reason: the agent is late")
        .coord(5, Kind::Info, "dropped: not needed\n")
        .build();
    // closed, then moved back by the owner: the ticket has not ended
    let reopened = Log::new(4)
        .assign(1)
        .done(2)
        .pass(3)
        .merged(4)
        .owner(5, "/crew move Ready")
        .push(
            6,
            COORD,
            Role::Coordinator,
            "owner",
            Kind::Info,
            "late: verify a/b#4\nlate-key: k1\n",
        )
        .build();
    let r = retro(&[merged, verified, dropped, reopened]);
    let f = &r.flow;
    assert_eq!((f.reassigned_tickets, f.reassigns.len()), (1, 2));
    assert_eq!(issues(&f.dropped.iter().collect::<Vec<_>>()), [3]);
    assert_eq!(
        f.closes,
        BTreeMap::from([("merged", 1), ("verified", 1), ("dropped", 1)])
    );
    assert_eq!(f.late_notes.len(), 1);
    // a late note is a report to the owner: of the coordinator or of a dev agent. The same
    // text to anyone else is no late note.
    let late = "late: verify a/b#5\nlate-key: k2\n";
    let notes = Log::new(5)
        .assign(1)
        .push(2, COORD, Role::Coordinator, "owner", Kind::Info, late)
        .dev(3, "owner", Kind::Info, late)
        .dev(4, "coordinator", Kind::Info, late)
        .coord(5, Kind::Info, late)
        .build();
    assert_eq!(retro(&[notes]).flow.late_notes.len(), 2);
}

const DROP_WITH_PR: &str = "dropped: merged without verify\npr: https://github.com/a/b/pull/7\nmerge-commit: abc\nmerged-by: crew-account\n";

#[test]
fn a_drop_with_a_merged_pr_is_a_drop_and_has_its_own_count() {
    // the text is the one that `close --why` writes
    assert_eq!(
        DROP_WITH_PR,
        crate::protocol::acts::dropped_merged_body(
            "merged without verify",
            "https://github.com/a/b/pull/7",
            Some("abc"),
            "crew-account"
        )
    );
    let plain = Log::new(1)
        .assign(1)
        .coord(2, Kind::Info, "dropped: not needed\n")
        .build();
    let with_pr = Log::new(2)
        .assign(1)
        .done(2)
        .coord(3, Kind::Info, DROP_WITH_PR)
        .build();
    // dropped with a PR, then moved back by the owner: the ticket has not ended
    let reopened = Log::new(3)
        .assign(1)
        .coord(2, Kind::Info, DROP_WITH_PR)
        .owner(3, "/crew move Ready")
        .build();
    // the same text of a dev agent is no drop
    let by_dev = Log::new(4)
        .assign(1)
        .dev(2, "coordinator", Kind::Info, DROP_WITH_PR)
        .build();
    // an older drop with a PR, then a new assignment and a plain drop: the end is a plain drop
    let later_plain = Log::new(5)
        .assign(1)
        .coord(2, Kind::Info, DROP_WITH_PR)
        .owner(3, "/crew move Ready")
        .assign(4)
        .coord(5, Kind::Info, "dropped: not needed\n")
        .build();
    let r = retro(&[plain, with_pr, reopened, by_dev, later_plain]);
    let f = &r.flow;
    assert_eq!(issues(&f.dropped.iter().collect::<Vec<_>>()), [1, 2, 5]);
    assert_eq!(issues(&f.dropped_merged.iter().collect::<Vec<_>>()), [2]);
    assert_eq!(f.dropped_merged[0].url, url(202));
    // it is a drop in every other number: no merge close, no time to a close
    assert_eq!(f.closes, BTreeMap::from([("dropped", 3)]));
    assert_eq!(f.done_to_close, None);
    assert_eq!((r.tickets, r.delivered), (5, 1));
    assert!(r.merged_unassigned.is_empty());
}

#[test]
fn merged_work_without_an_assignment_is_in_one_number_only() {
    let ticket = Log::new(1).assign(1).done(2).pass(3).merged(4).build();
    // no assignment: the coordinator drops the issue and records the merged PR
    let unassigned = Log::new(2)
        .coord(1, Kind::Info, "Ticket is on the board as PVTI_x.\n")
        .coord(2, Kind::Info, DROP_WITH_PR)
        .build();
    // not counted: a plain drop, the text of a dev agent, and a drop that the owner moved back
    let plain = Log::new(3)
        .coord(1, Kind::Info, "dropped: not needed\n")
        .build();
    let by_dev = Log::new(4)
        .dev(1, "coordinator", Kind::Info, DROP_WITH_PR)
        .build();
    let reopened = Log::new(5)
        .coord(1, Kind::Info, DROP_WITH_PR)
        .owner(2, "/crew move Backlog")
        .build();
    let alone = retro(std::slice::from_ref(&ticket));
    let r = retro(&[ticket, unassigned, plain, by_dev, reopened]);
    assert_eq!(issues(&r.merged_unassigned.iter().collect::<Vec<_>>()), [2]);
    assert_eq!(r.merged_unassigned[0].url, url(201));
    // every other number is the one of the ticket alone
    assert_eq!(
        Report {
            merged_unassigned: Vec::new(),
            ..r
        },
        alone
    );
}

#[test]
fn the_flow_times_are_the_median_and_the_ninetieth_percentile_over_tickets() {
    assert_eq!(spread(&[]), None);
    assert_eq!(
        spread(&[7]),
        Some(Spread {
            tickets: 1,
            median_s: 7,
            p90_s: 7
        })
    );
    // an even count: the mean of the two middle values
    let s = spread(&[40, 10, 30, 20]).unwrap();
    assert_eq!((s.median_s, s.p90_s), (25, 40));
    // ten values: the ninth is the 90th percentile; eleven: the tenth
    let ten: Vec<i64> = (1..=10).collect();
    let s = spread(&ten).unwrap();
    assert_eq!((s.tickets, s.median_s, s.p90_s), (10, 5, 9));
    let eleven: Vec<i64> = (1..=11).collect();
    let s = spread(&eleven).unwrap();
    assert_eq!((s.median_s, s.p90_s), (6, 10));

    // assignment to the FIRST done, first done to the close; a ticket without a done or
    // without a close has no time, and a drop is no close of delivered work
    let a = Log::new(1).assign(0).done(2).pass(3).merged(5).build();
    let b = Log::new(2)
        .assign(0)
        .done(4)
        .fail(5, &[review("x")])
        .done(6)
        .pass(7)
        .merged(13)
        .build();
    let c = Log::new(3)
        .assign(0)
        .done(9)
        .coord(10, Kind::Info, "dropped: not needed\n")
        .build();
    let d = Log::new(4).assign(0).build();
    let r = retro(&[a, b, c, d]);
    assert_eq!(
        r.flow.assign_to_done,
        Some(Spread {
            tickets: 3,
            median_s: 4 * HOUR,
            p90_s: 9 * HOUR
        })
    );
    assert_eq!(
        r.flow.done_to_close,
        Some(Spread {
            tickets: 2,
            median_s: 6 * HOUR,
            p90_s: 9 * HOUR
        })
    );
}

/// An assignment whose signed gate line is `text`.
fn assign_gate(log: Log, hour: i64, text: &str) -> Log {
    log.coord(
        hour,
        Kind::Request,
        &format!("Assigned to you: t\ngate: {text}\n"),
    )
}

#[test]
fn the_tickets_are_counted_by_the_kind_of_their_signed_gate() {
    // 1: Gate `ci`, first verify passes. 2: Gate `CI ` (any case, spaces), first verify fails.
    // 3: another Gate, passes. 4: another Gate, not verified yet. 5: no gate line at all.
    let logs = vec![
        assign_gate(Log::new(1), 1, "ci").done(2).pass(3).build(),
        assign_gate(Log::new(2), 1, " CI ")
            .done(2)
            .fail(3, &[review("x")])
            .done(4)
            .pass(5)
            .build(),
        assign_gate(Log::new(3), 1, "cargo test")
            .done(2)
            .pass(3)
            .build(),
        assign_gate(Log::new(4), 1, "ci and a review")
            .done(2)
            .build(),
        Log::new(5)
            .coord(1, Kind::Request, "Assigned to you: t\n")
            .done(2)
            .pass(3)
            .build(),
    ];
    let g = retro(&logs).gates;
    let row = |tickets, verified, first_pass| GateRow {
        tickets,
        verified,
        first_pass,
    };
    assert_eq!(g.ci_only, row(2, 2, 1));
    assert_eq!(g.other, row(2, 1, 1));
    assert_eq!(g.unsigned, row(1, 1, 1));
}

#[test]
fn the_latest_assignment_decides_the_gate_kind() {
    let log = assign_gate(Log::new(1), 1, "make check");
    let log = assign_gate(log, 2, "ci").done(3).pass(4).build();
    let g = retro(&[log]).gates;
    assert_eq!((g.ci_only.tickets, g.other.tickets), (1, 0));
}

#[test]
fn a_ticket_that_went_back_two_times_is_counted_once() {
    let logs = vec![
        // two fails that are rework
        Log::new(1)
            .assign(1)
            .done(2)
            .fail(3, &[review("a")])
            .done(4)
            .fail(5, &[review("b")])
            .done(6)
            .pass(7)
            .build(),
        // one fail
        Log::new(2)
            .assign(1)
            .done(2)
            .fail(3, &[review("a")])
            .done(4)
            .pass(5)
            .build(),
        // none
        Log::new(3).assign(1).done(2).pass(3).build(),
    ];
    let y = retro(&logs).verify;
    assert_eq!((y.rework_tickets, y.repeat_rework_tickets), (2, 1));
}

#[test]
fn the_passes_that_the_tool_decided_are_counted_apart_from_typed_ones() {
    let logs = [
        Log::new(1).assign(1).done(2).tool_pass(3).build(),
        Log::new(2).assign(1).done(2).typed_pass(3).build(),
        Log::new(3).assign(1).done(2).pass(3).build(),
        // a fail, then a pass of the tool
        Log::new(4)
            .assign(1)
            .done(2)
            .fail(3, &[review("x")])
            .done(4)
            .tool_pass(5)
            .build(),
    ];
    let y = retro(&logs).verify;
    assert_eq!((y.passes, y.tool_run_passes), (4, 2));
}

#[test]
fn review_fails_are_counted_by_the_tag_of_their_note_and_three_of_one_tag_repeat() {
    let fmt = || review("[formatter] cargo fmt was not run");
    let logs = [
        Log::new(1).assign(1).done(2).fail(3, &[fmt()]).done(4).pass(5).build(),
        Log::new(2).assign(1).done(2).fail(3, &[fmt()]).build(),
        Log::new(3)
            .assign(1)
            .done(2)
            .fail(3, &[fmt()])
            .done(4)
            .fail(5, &[review("[scope] a file outside the scope")])
            .build(),
        // no tag, a machine class (not a review fail), and a failed check of the tool
        Log::new(4).assign(1).done(2).fail(3, &[review("part 2 is missing")]).build(),
        Log::new(5).assign(1).done(2).fail(3, &[red()]).build(),
        Log::new(6)
            .assign(1)
            .done(2)
            .fail(
                3,
                &[review(
                    "the tool ran the checks of the Gate: check tests exited 1 on a clean checkout of abc",
                )],
            )
            .build(),
    ];
    let t = retro(&logs).tags;
    let rows: Vec<(&str, usize)> = t
        .rows
        .iter()
        .map(|r| (r.tag.as_str(), r.events.len()))
        .collect();
    assert_eq!(rows, [("formatter", 3), ("check-tests", 1), ("scope", 1)]);
    assert_eq!(t.untagged, 1);
    assert!(t.rows[0].events.len() >= REPEATED_FROM);
}

const MERGE_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

/// The notice of `merge` for PR `pr` of the repo a/b, with the merge commit [`MERGE_SHA`].
fn merge_note(pr: u64) -> String {
    crate::protocol::acts::merged_body(
        &format!("https://github.com/a/b/pull/{pr}"),
        "abc",
        "squash",
        MERGE_SHA,
        "crew/x",
        None,
    )
}

fn commit(repo: &str, hour: i64, message: &str) -> BaseCommit {
    BaseCommit {
        repo: repo.into(),
        sha: format!("c{hour}"),
        at: hour * HOUR,
        message: message.into(),
    }
}

#[test]
fn a_merged_ticket_that_opens_again_came_back_and_a_reopened_drop_did_not() {
    let merged = || Log::new(1).assign(0).done(1).pass(2).merged(3);
    let came_back = |log: Log| retro(&[log.build()]).outcome.came_back;
    // control: a merged ticket that stays closed
    let r = retro(&[merged().build()]);
    assert_eq!((r.outcome.merged, r.outcome.came_back.len()), (1, 0));
    // the owner moves it back: the event is the message that opened it
    let moved = came_back(merged().owner(5, "/crew move Ready"));
    assert_eq!(moved.len(), 1);
    assert_eq!((moved[0].url.as_str(), moved[0].at), (&*url(104), 5 * HOUR));
    // a plain owner comment and a message of the dev agent open nothing
    assert!(came_back(merged().owner(5, "thanks").done(6)).is_empty());
    // a new assignment, a reassign and a failed verify of the coordinator open it too
    assert_eq!(came_back(merged().assign(5))[0].url, url(104));
    let reassigned = merged().coord(5, Kind::Info, "Reassigned. Reason: a fault showed up");
    assert_eq!(came_back(reassigned).len(), 1);
    assert_eq!(came_back(merged().fail(5, &[review("broken")])).len(), 1);
    // opened two times: one ticket, the first message
    let twice = merged().owner(5, "/crew move Ready").merged(6).assign(7);
    assert_eq!(came_back(twice)[0].url, url(104));
    // a fail before the merge is the normal flow
    let rework = Log::new(1)
        .assign(0)
        .done(1)
        .fail(2, &[review("x")])
        .done(3);
    assert!(came_back(rework.pass(4).merged(5)).is_empty());

    // a reopened drop is a changed decision: no merged ticket, nothing came back
    let drop = Log::new(2)
        .assign(0)
        .coord(1, Kind::Info, "dropped: not needed\n")
        .owner(2, "/crew move Ready")
        .assign(3);
    let r = retro(&[drop.build()]);
    assert_eq!((r.outcome.merged, r.outcome.came_back.len()), (0, 0));
    // a drop after a merge: the ticket is a merged ticket, and it came back
    let merged_then_dropped = merged().owner(5, "/crew move Ready").coord(
        6,
        Kind::Info,
        "dropped: the change was wrong\n",
    );
    let r = retro(&[merged_then_dropped.build()]);
    assert_eq!((r.outcome.merged, r.outcome.came_back.len()), (1, 1));
    // a drop whose note records a merged PR is merged work too
    let drop_with_pr = Log::new(3).assign(0).coord(1, Kind::Info, DROP_WITH_PR);
    let r = retro(&[drop_with_pr.owner(2, "/crew move Ready").build()]);
    assert_eq!((r.outcome.merged, r.outcome.came_back.len()), (1, 1));
    // an issue without an assignment is in no outcome number
    let unassigned = Log::new(4).merged(1).owner(2, "/crew move Ready");
    assert_eq!(retro(&[unassigned.build()]).outcome, Outcome::default());
}

#[test]
fn the_owner_numbers_count_the_owner_comments_and_the_wait_for_an_answer() {
    // two questions, the owner answers both with one comment: waits of 3 h and 1 h
    let answered = Log::new(1)
        .assign(0)
        .dev(1, "owner", Kind::Question, "which API?")
        .dev(3, "owner", Kind::Question, "and which version?")
        .owner(4, "the new one, v2")
        .owner(5, "thanks");
    // the coordinator withdraws the question after 6 h; its plain answer closes nothing
    let withdrawn = Log::new(2)
        .assign(0)
        .dev(2, "owner", Kind::Question, "may I drop the cache?")
        .coord(3, Kind::Answer, "I think so")
        .coord(
            8,
            Kind::Answer,
            "not needed any more\nwithdraws: question\n",
        );
    // an open question is in no wait; a question to the coordinator is none to the owner
    let open = Log::new(3)
        .assign(0)
        .dev(1, "coordinator", Kind::Question, "which branch?")
        .owner(2, "/crew move Ready")
        .dev(3, "owner", Kind::Question, "still open?");
    let untouched = Log::new(4).assign(0);
    // an owner comment on an issue without an assignment counts nowhere
    let no_ticket = Log::new(5)
        .dev(1, "owner", Kind::Question, "early")
        .owner(2, "no");
    let r = retro(&[
        answered.build(),
        withdrawn.build(),
        open.build(),
        untouched.build(),
        no_ticket.build(),
    ]);
    let o = &r.outcome;
    assert_eq!((r.tickets, o.owner_touched), (4, 2));
    // the touched tickets have 2 and 1 owner comments
    assert_eq!(
        o.owner_comments,
        Some(Spread {
            tickets: 2,
            median_s: 1,
            p90_s: 2
        })
    );
    // the waits are 1 h, 3 h and 6 h
    assert_eq!(
        o.owner_wait,
        Some(Spread {
            tickets: 3,
            median_s: 3 * HOUR,
            p90_s: 6 * HOUR
        })
    );
    assert_eq!(r.andon.questions_owner.len(), 4);
    assert_eq!(r.andon.open_questions.len(), 1);
}

#[test]
fn the_merge_refs_are_the_pr_and_the_merge_commit_of_the_close_notes() {
    let merged = Log::new(1).assign(0).coord(3, Kind::Info, &merge_note(7));
    // an owner merge whose commit GitHub did not name: the PR alone
    let owner_merged = crate::protocol::acts::owner_merged_body(
        "https://github.com/a/c/pull/9",
        None,
        Some("pr: https://github.com/x/y/pull/1"),
    );
    let by_owner = Log::new(2).assign(0).coord(4, Kind::Info, &owner_merged);
    let drop = Log::new(3).assign(0).coord(5, Kind::Info, DROP_WITH_PR);
    // no ref: a merge note without the lines, a plain drop, a note on an issue without an
    // assignment, and the note text of a dev agent
    let bare = Log::new(4).assign(0).merged(1);
    let plain_drop = Log::new(5).assign(0).coord(1, Kind::Info, "dropped: no\n");
    let unassigned = Log::new(6).coord(1, Kind::Info, &merge_note(8));
    let by_dev = Log::new(7)
        .assign(0)
        .dev(1, "coordinator", Kind::Info, &merge_note(8));
    let logs = [
        merged.build(),
        by_owner.build(),
        drop.build(),
        bare.build(),
        plain_drop.build(),
        unassigned.build(),
        by_dev.build(),
    ];
    let refs = merged_refs(&logs);
    let short: Vec<_> = refs
        .iter()
        .map(|r| {
            (
                r.close.ticket.issue,
                r.close.at / HOUR,
                r.repo.as_str(),
                r.pr,
                r.merge_commit.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        short,
        [
            (1, 3, "a/b", Some(7), Some(MERGE_SHA)),
            (2, 4, "a/c", Some(9), None),
            // the sha `abc` of the drop note is too short to be compared
            (3, 5, "a/b", Some(7), None),
        ]
    );
    // the ticket without a ref is a merged ticket, but no revert can be found for it
    let o = super::retro(&logs, Some(&[])).outcome;
    assert_eq!((o.merged, o.reverted.known, o.reverted.read), (4, 3, true));
    assert!(o.reverted.reverts.is_empty());
    // without the commits the count says nothing
    let o = retro(&logs).outcome;
    assert_eq!((o.reverted.known, o.reverted.read), (3, false));
}

#[test]
fn a_revert_matches_by_the_sha_a_prefix_of_it_or_the_pr_number_of_the_same_repo() {
    let log = || {
        [Log::new(1)
            .assign(0)
            .coord(3, Kind::Info, &merge_note(7))
            .build()]
    };
    let reverted = |c: &[BaseCommit]| super::retro(&log(), Some(c)).outcome.reverted.reverts;
    let by_sha = format!("Revert \"t\"\n\nThis reverts commit {MERGE_SHA}.\n");
    let hit = reverted(&[commit("a/b", 9, &by_sha)]);
    assert_eq!(hit.len(), 1);
    // the event is the close note; the commit is the revert
    assert_eq!(hit[0].close.url, url(101));
    assert_eq!((hit[0].sha.as_str(), hit[0].at), ("c9", 9 * HOUR));
    // a prefix of 12 hex digits, in upper case
    let prefix = format!("This reverts commit {}", MERGE_SHA[..12].to_uppercase());
    assert_eq!(reverted(&[commit("a/b", 9, &prefix)]).len(), 1);
    // 11 digits are too few, and another sha is another commit
    let short = format!("This reverts commit {}", &MERGE_SHA[..11]);
    assert!(reverted(&[commit("a/b", 9, &short)]).is_empty());
    let other = format!("This reverts commit {}0", &MERGE_SHA[..39][1..]);
    assert!(reverted(&[commit("a/b", 9, &other)]).is_empty());
    // the PR number, with and without the repo
    for text in [
        "Revert \"t\"\n\nReverts a/b#7",
        "Reverts #7\n",
        "  Reverts A/B#7.",
    ] {
        assert_eq!(reverted(&[commit("a/b", 9, text)]).len(), 1, "{text}");
    }
    // not the PR of another repo, another number, or a word that only starts like one
    for text in [
        "Reverts a/c#7",
        "Reverts #70",
        "Reverts #7a",
        "Reverts b#7",
        "This Reverts #7",
        "mentions #7 and a/b#7",
    ] {
        assert!(reverted(&[commit("a/b", 9, text)]).is_empty(), "{text}");
    }
    // a commit of another repo names its own PR 7 and its own commits
    assert!(reverted(&[commit("a/c", 9, "Reverts #7")]).is_empty());
    assert!(reverted(&[commit("a/c", 9, &by_sha)]).is_empty());
    // two reverts of one ticket: one ticket, the earliest commit
    let two = [commit("a/b", 9, "Reverts #7"), commit("a/b", 6, &by_sha)];
    let hit = reverted(&two);
    assert_eq!((hit.len(), hit[0].sha.as_str()), (1, "c6"));
}
