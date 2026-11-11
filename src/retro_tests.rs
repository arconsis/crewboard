use super::*;
use crate::protocol::acts::{EvidenceMsg, VerifyFailure, done_body, verify_body};
use crate::testkit::agent;

const COORD: &str = "agent-a@host-a";
const DEV: &str = "agent-b@host-b";
const OTHER_DEV: &str = "agent-c@host-c";
const HOUR: i64 = 3600;

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
            tickets_with_fail: 2,
            fails: 3,
            rework_tickets: 2,
            rework_fails: 3,
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

#[test]
fn a_head_moved_fail_of_a_ticket_that_is_merged_with_no_further_done_is_no_rework() {
    // accepted: the next step after the fail is a close that says "merged"
    let accepted = Log::new(1)
        .assign(1)
        .done(2)
        .fail(3, &[head_moved()])
        .coord(
            4,
            Kind::Info,
            "The ticket is closed: the owner merged the PR by hand.\n",
        )
        .build();
    // rework: the dev agent delivered again
    let redone = Log::new(2)
        .assign(1)
        .done(2)
        .fail(3, &[head_moved()])
        .done(4)
        .pass(5)
        .merged(6)
        .build();
    // rework: the fail has a second class
    let two = Log::new(3)
        .assign(1)
        .done(2)
        .fail(3, &[head_moved(), red()])
        .merged(4)
        .build();
    // not accepted: the ticket was dropped, or is still open
    let dropped = Log::new(4)
        .assign(1)
        .done(2)
        .fail(3, &[head_moved()])
        .coord(4, Kind::Info, "dropped: not needed\n")
        .build();
    let open = Log::new(5)
        .assign(1)
        .done(2)
        .fail(3, &[head_moved()])
        .build();
    // accepted: a second head-moved fail before the merge is accepted too
    let twice = Log::new(6)
        .assign(1)
        .done(2)
        .fail(3, &[head_moved()])
        .fail(4, &[head_moved()])
        .coord(5, Kind::Info, "Closed after a passed verify\n")
        .build();
    let r = retro(&[accepted, redone, two, dropped, open, twice]);
    assert_eq!(issues(&class(&r, ACCEPTED_AFTER_HEAD_MOVED)), [1, 6, 6]);
    assert_eq!(issues(&class(&r, "head-moved")), [2, 3, 4, 5]);
    assert_eq!(
        (r.verify.tickets_with_fail, r.verify.fails),
        (6, 7),
        "all fails"
    );
    assert_eq!(
        (r.verify.rework_tickets, r.verify.rework_fails),
        (4, 4),
        "without the accepted fails"
    );
    // the rounds leave the accepted fails out
    assert_eq!(
        r.rounds.iter().map(|x| x.ticket.issue).collect::<Vec<_>>(),
        [2, 3, 4, 5]
    );
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
        .coord(6, Kind::Info, "late: verify a/b#4\nlate-key: k1\n")
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
